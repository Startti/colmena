# Memoria por interacción y mapas de resultados estructurados — Design

**Fecha:** 2026-08-22
**Estado:** diseño cerrado en brainstorm, sin implementar
**Reemplaza:** la degradación del mensaje más nuevo (`degrade_newest_tool_result`), removida del
árbol el 2026-08-22 antes de llegar a `develop`
**Relacionado:** [Item 14 del backlog](../../BACKLOG.md) ("Output filtering para LLM — qué campos ve
el modelo"), [spec de resumen semántico](2026-06-18-conversation-semantic-summary-design.md)

---

## Problema

Dos costos distintos, medidos por el owner sobre agentes reales:

1. **Costo en tokens.** Un resultado de tool de 300 filas entra crudo al prompt y se paga en cada
   turno mientras siga en la ventana reciente.
2. **Calidad de respuesta.** El modelo recibe 300 filas cuando importaban 3, y contesta peor que si
   hubiera recibido menos y mejor ordenado.

El exceso viene en **dos formas que pesan parecido**: estructural (un `SELECT *` que trae 20
columnas cuando importan 3, una API que devuelve un objeto enorme del que se usan dos campos) y
semántica (texto largo donde lo relevante está enterrado y no hay campo que lo identifique).

Y hay un tercer problema, estructural, que salió al investigar: **el borde de la ventana reciente lo
decide únicamente el presupuesto de tokens.** Un resultado de tool grande empuja ese borde hasta
comerse la pregunta que el usuario acaba de hacer, que termina resumida a 250 caracteres mientras el
resultado viaja entero. El modelo recibe todos los datos y pierde el criterio para leerlos.

## Principio rector

> Todo lo que llega nuevo —de una tool, de una persona, de otro agente— tiene que llegarle al modelo
> al 100%. Recién cuando pasa a ser memoria, es decir cuando llega el mensaje de otra interacción,
> se puede resumir.

El motivo no es purismo: el modelo solo puede responder con lo que ve. `recall_history` existe, pero
depende de que el modelo **se dé cuenta** de que le falta algo — y un resumen bien escrito no se
siente incompleto. Es una red que solo atrapa a quien sabe que se está cayendo.

## Objetivo

Bajar el consumo de tokens **sin** que un modelo decida qué información pierde otro modelo.

El ahorro sale de tres lugares, en este orden de preferencia:

1. **No traer lo que no se necesita** (proyección al llamar la tool).
2. **Describir en vez de transcribir** lo que igual llegó grande (el mapa).
3. **Comprimir lo que ya es pasado** (la zona de memoria, que ya funciona y no se toca).

## No-objetivos (v1)

- **Extracción con modelo barato sobre bloques opacos de texto.** Diferido a v1.1, ver
  [Diferido](#diferido-capa-2--extracción-de-prosa).
- Cambiar el resumen semántico de la zona de memoria. Funciona y se mantiene tal cual.
- Poner un tope duro al tamaño del request. No hay ninguno hoy en el módulo LLM (todos los
  `max_tokens` que existen son de salida) y este diseño no agrega uno.

> El pinneo posicional de `messages[..SUMMARY_KEEP_FIRST_MSGS]` **sí entra** en el alcance: pasa a
> ser por rol. Ver [Zona 0](#zona-0--system).

---

## Decisiones cerradas en brainstorm

| # | Decisión | Alternativa descartada y por qué |
|---|---|---|
| 1 | El `system` va completo, siempre | — |
| 2 | La zona de memoria queda como está | — |
| 3 | La "ventana reciente" y la "cola viva" se funden en una sola zona: **la interacción actual** | Mantenerlas separadas: la separación de hoy es un accidente (una vino de la base, la otra del proceso), no una diferencia real |
| 4 | El borde entre memoria e interacción actual es **estructural**, no presupuestario | Seguir cortando por presupuesto: es lo que causa que la pregunta del usuario caiga en memoria |
| 5 | Lo estructurado se **mapea**, nunca se resume ni se trunca | Recortar a N caracteres: `bridge_truncate` corta a mitad de un registro JSON y entrega basura sintáctica |
| 6 | La salida de emergencia para lo estructurado es **`data_run_python`**, no devolver filas al prompt | `inspect_result`: devuelve *más datos*; `data_run_python` devuelve *una respuesta*, calculada sobre el original completo |
| 7 | La cantidad de datos crudos **se degrada gradualmente** | Switch duro: un resultado apenas sobre el umbral perdería casi todo y forzaría un turno innecesario |
| 8 | Un resultado anidado no tabular **falla con mensaje claro** | Aplanar con `json_normalize`: inventa nombres de columna que el modelo no vio en el mapa y expande arrays en silencio |
| 9 | Se guarda **siempre el retorno exacto** | Guardar lo que el modelo consideró importante destruye el original y convierte `recall_history` en una mentira |
| 10 | Los errores nunca se mapean | — |
| 11 | La capa 2 (prosa) queda afuera de v1 y se decide con datos | Implementarla ahora: economía sin demostrar, ver [Diferido](#diferido-capa-2--extracción-de-prosa) |

---

## Arquitectura — tres zonas

Hoy el prompt tiene cuatro zonas: `messages[..keep_first]` verbatim, la zona de memoria resumida,
la ventana reciente verbatim acotada por presupuesto, y la cola viva verbatim sin tope. Este diseño
las reduce a tres.

### Zona 0 — `system`

Completo, siempre, nunca compactado. **Y nada más.**

Hoy el pinneo es **posicional**: `messages[..SUMMARY_KEEP_FIRST_MSGS]` con `SUMMARY_KEEP_FIRST_MSGS
= 2`, lo que fija los dos primeros mensajes almacenados verbatim en todos los turnos, para siempre.
Eso arrastra el primer mensaje del usuario, que no tiene por qué estar pinneado: si alguien abre el
chat pegando un documento de 100k tokens, se paga en cada llamada de esa conversación.

**Decisión: el pinneo pasa a ser por ROL, no por posición.** Se conserva el mensaje `system` (las
instrucciones, que son las que dicen cómo debe funcionar el agente) y nada más.

**Por qué por rol y no simplemente `keep_first = 1`:** el orden almacenado no garantiza que el
`system` esté en la posición 0. En un dump real del wire (`COLMENA_DUMP_PROMPT_FULL=1`, 2026-08-22)
el `system` estaba en la **posición 1**, después de un mensaje de usuario. Con `keep_first = 1` se
pinnearía el mensaje del usuario y se perdería el pinneo de las instrucciones — exactamente al
revés de la intención.

**A verificar en la implementación:** el orden exacto en que `llm.rs` ensambla y persiste
`params.messages` frente al `prompt` (`agent_service.rs` los trata en ramas `if/else if`, no
acumulativas). La regla por rol es correcta independientemente de ese orden, pero el ensamblado hay
que dejarlo entendido y cubierto por un test antes de tocar el pinneo.

**Dependencia real:** hoy el bloque de resumen se anexa como un **segundo** mensaje `system` cuando
`messages[keep_first - 1]` no es `System`, y el adaptador de Anthropic colapsa múltiples `system`
por **sobrescritura** (gana el último). Es decir que en Anthropic esta garantía **no se cumple hoy**:
cualquier conversación lo bastante larga como para compactarse pierde el system prompt real del
agente. Está tomado como trabajo aparte y este diseño depende de que se cierre.

### Zona 1 — Memoria

Todo lo que pertenece a interacciones **cerradas**. Sin cambios respecto de hoy: resumen semántico
cacheado en la columna `summary` de `llm_node_history`, digest determinista para tool-results
estructurados, línea estructural para `assistant` con `tool_calls`, marcadores para andamiaje
(`load_skill`, `describe_tool`), tope de `SUMMARY_MAX_LINES` líneas, y recuperación verbatim vía
`recall_history(turn=N)`.

Esta es la zona que paga el ahorro, y es la única donde se pierde fidelidad en el prompt.

### Zona 2 — Interacción actual

Todo lo que va desde el arranque de la interacción abierta hasta el final, **sin importar si vino de
la base (resume) o se generó en esta corrida**. Verbatim por defecto.

Lo único que se comprime acá son resultados de tool individuales que no entran en su asignación, y
se comprimen **describiéndolos** (el mapa), nunca seleccionando qué parte es relevante.

**Qué pasa cuando la interacción cierra.** El borde se mueve y todo lo que estaba en la zona 2 pasa
a la zona 1 en el siguiente load: los mensajes se resumen y los tool-results estructurados reciben
el digest compacto de siempre. El mapa es un artefacto **de la interacción abierta**, no un estado
persistente — nunca se guarda ni sobrevive al cierre.

### Qué pasa con el presupuesto de tokens

Deja de gobernar **dónde se corta la conversación**: ese corte pasa a ser estructural. Lo que decide
cuánto de un resultado individual viaja crudo antes de que aparezca el mapa es un valor distinto,
con otro nombre y otro trabajo: la **asignación por resultado**.

**No es la misma constante reciclada, y la distinción importa.** `RECENT_TOKEN_BUDGET` es hoy un
`pub const` global sin ningún mecanismo de override; la asignación por resultado es un valor nuevo,
overridable por el operador en la config del `llm_call` (ver
[Cuánto vale y quién la configura](#degradación-gradual--una-sola-perilla)). Reusar la constante
vieja para el concepto nuevo ataría dos decisiones distintas al mismo número: cambiar cuánto de un
resultado viaja crudo movería también cualquier otro comportamiento que siga leyendo esa constante.

El cambio no es cosmético: un presupuesto que corta conversaciones puede dejar afuera la pregunta
del usuario. Una asignación que decide cuánto de un resultado va crudo no puede hacer eso nunca.

---

## El borde de interacción

**Regla:** la interacción actual arranca justo después del último mensaje `assistant` que **no trae
tool calls**.

### Por qué es estructural y no una heurística

El loop ReAct de `agent_service.rs` tiene exactamente tres salidas, y las tres se apoyan en la misma
condición:

| Condición | Qué hace | Ref |
|---|---|---|
| `tool_calls() == Some(no vacío)` | ejecuta las tools y `continue` | condición en `:354`, `continue` en `:670` |
| `tool_calls() == Some(vacío)` | `return` — cierra el turno | condición en `:354`, `return` en `:360` |
| `tool_calls() == None` | `return` — cierra el turno | `else` en `:671`, `return` en `:677` |

(Todas en `src/libs/colmena/src/llm/application/agent_service.rs`. Se citan **dos** líneas por fila
—la condición y la sentencia que cierra— porque están a cientos de líneas de distancia y citar solo
una manda al lector al lugar equivocado.)

El loop termina **si y solo si** el assistant no trajo tool calls. Un `assistant` persistido sin
tool calls es, por construcción, el cierre de una interacción.

Las otras dos salidas del nodo confirman la regla:

- **Rescue** (`agent_service.rs:681`): al agotarse el techo de turnos o saltar el loop-guard, hace
  una llamada final **sin tools en el request**, así que el modelo no puede pedir tool calls. Lo que
  persiste es necesariamente un assistant sin tool calls. Cierra bien. (El mensaje sintético que el
  rescue empuja al vector local **no** se persiste, así que no ensucia el historial.)
- **Suspend** (`agent_service.rs:501`): devuelve a mitad del loop con tool calls pendientes. No
  queda ningún assistant de cierre, así que la interacción sigue abierta y al reanudar todo eso cae
  en la zona 2 y viaja verbatim.

### Dos trampas de implementación

1. **`Some(vacío)` y `None` son dos casos, no uno.** Ambas salidas existen porque el streaming
   reconstruye las tool calls por separado. Detectar el cierre con `tool_calls().is_none()` sería un
   bug: se comería el caso `Some(vec![])` y nunca cerraría una interacción en el camino
   no-streaming. La condición correcta es "no hay tool calls" en sentido amplio.
2. **El camino con streaming necesita test propio.** Las tool calls se acumulan por deltas y se
   arman al final. Si esa reconstrucción fallara, un assistant *con* tool calls se persistiría *sin*
   ellas y la regla cerraría una interacción que sigue abierta. Es el único modo en que esta regla
   puede mentir.

### Caso de verificación

Historial `[…, Assistant(final), User, Assistant(tool_calls), Tool(26k)]` — la forma que reportó
ADP. El último assistant de cierre es `Assistant(final)`, así que la zona 2 arranca en el `User`:
la pregunta, la llamada y el resultado de 26k viajan **los tres verbatim**.

---

## El mapa

### Cuándo se activa

Cinco condiciones simultáneas:

1. El resultado está en la zona 2.
2. **No entra en su asignación** (ver abajo — esto reemplaza al umbral como perilla).
3. El analizador reconoce estructura. Si no, es prosa: fuera de v1.
4. El agente tiene `data_run_python`. **Sin salida de emergencia no hay mapa** — un mapa que
   promete un instrumento inexistente es peor que mandar los datos crudos.
5. `ToolResult.success == true`.

### Degradación gradual — una sola perilla

Hay **una** asignación por resultado, en tokens. El comportamiento sale solo:

- Si el resultado crudo entra en la asignación → **va entero, sin encabezado**. Idéntico a hoy, sin
  overhead.
- Si no entra → aparece el mapa, y el resto de la asignación se llena con **tantas filas crudas como
  quepan**, más el aviso de cuántas quedaron afuera.

Así un resultado mediano llega con 50 filas y el modelo contesta sin turno extra; uno gigante llega
con 5. El escalón desaparece y el costo del turno extra aparece solo cuando el dato es realmente
inmanejable. No hay umbral separado que ajustar.

**Cuánto vale y quién la configura.** Es la **única** perilla del diseño: una constante en tokens,
overridable por el operador en la config del `llm_call`. El valor inicial se elige **provisorio y se
mide** — no se deriva de ninguna teoría. `COLMENA_DUMP_PROMPT_SIZES` ya vuelca el tamaño por mensaje
en corridas reales, así que la calibración es observar agentes en uso y ajustar, no adivinar. Punto
de partida sugerido para la primera medición: el orden de `RECENT_TOKEN_BUDGET` (2.500 tokens), por
ser el número con el que el sistema ya venía razonando; sin ninguna pretensión de que sea el
correcto.

### Contenido

Sobre el digest actual, dos agregados que se pagan solos:

- **Tipos y nulos por columna.** No es cosmético: es lo que hace que el modelo escriba el pandas
  bien **a la primera**. Sin saber que `fecha` viene como texto `"2026-01-03"` y no como timestamp,
  o que `monto` tiene 12 nulos, el primer intento falla, el error vuelve y se pierde un turno
  entero — justo lo que se venía a ahorrar.
- **Las filas de muestra van crudas, sin tocar.** Su trabajo cambió: ya no son "una muestra
  representativa para contestar", son **ejemplares de formato para escribir código correcto**.
  Embellecerlas o normalizarlas destruye exactamente su utilidad.

Por la misma razón, **una muestra "inteligente"** (cubrir extremos y categorías) **no aporta** y
queda descartada: la representatividad dejó de ser el punto, y una muestra curada además se *siente*
completa, lo que reduce la probabilidad de que el modelo tire del hilo cuando debería.

Ejemplo del bloque que recibe el modelo:

```text
[sql_query · 300 filas · 6 columnas]
columnas:  id (entero) · fecha (texto ISO) · cliente (texto) · region (texto) ·
           monto (decimal, 12 nulos) · estado (texto)
rangos:    monto 1.200 – 480.000 · fecha 2026-01-03 – 2026-08-19
distintos: region {norte, sur, este, oeste} · estado {pagada, pendiente, vencida}
filas (crudas, 48 de 300):
  {"id":1,"fecha":"2026-01-03","cliente":"ACME","region":"norte","monto":12000,"estado":"pagada"}
  …
[252 filas omitidas — para calcular sobre el total:
 data_run_python(bindings=[{"var":"v","from_tool_call":"call_d7cd8dfb"}], code=…)]
```

### Un analizador, dos renderizados

El análisis es uno solo —contar, tipar, rangear, muestrear— y se renderiza a dos niveles:
**compacto** para la zona de memoria (`digest_tool_result` como hoy) y **rico** para la zona 2.
Mismo motor, dos salidas: evita que se bifurquen dos verdades sobre el mismo dato.

### Punto de aplicación — no alcanza con `build_compacted_messages`

`build_compacted_messages` corre **una sola vez, al arrancar la corrida**. Los resultados de tool
generados *durante* la corrida nunca pasan por ahí (por eso la cola viva es verbatim hoy).

El mapa necesita un **segundo punto de aplicación**: dentro del loop, donde se construye el
`tool_message` (`agent_service.rs:658`). Ahí resulta además más limpio — se **persiste el original**
y se pone **el mapa** en el vector local, en una sola operación.

---

## El binding `from_tool_call`

Un quinto discriminador estructural en `data_run_python`, junto a `attachment_id`,
`spreadsheet_id`+`sheet`, `query` y `data`:

```json
{"var": "ventas", "from_tool_call": "call_d7cd8dfb"}
```

**Por qué `tool_call_id` y no número de turno:** el índice depende de la posición en el historial y
el mapa se genera al crear el mensaje, cuando esa posición todavía no importa. El `tool_call_id` ya
está en el mensaje, es único y es estable. `recall_history` puede seguir usando turnos.

**Resolución:** busca el mensaje `tool` persistido con ese `tool_call_id`, parsea el contenido y lo
normaliza a records reutilizando el normalizador del binding INLINE, que recibe exactamente esa
forma. El sandbox lo recibe como cualquier otro binding.

**Alcance — requisito de seguridad, no comodidad:** solo tool calls de **la misma conversación**. Un
binding que pudiera alcanzar el historial de otro agente o de otra sesión sería un camino de
exfiltración. La clave de conversación ya delimita esto; hay que usarla y **testearlo
explícitamente**.

**Gating:** participa del gating de fuentes por configuración del operador que el tool ya tiene.

**Errores estructurados:** si el `tool_call_id` no existe o el contenido no se puede normalizar a
filas, vuelve un error con las mismas ayudas de auto-corrección que el tool ya trae (`loaded_columns`
y compañía).

**Anidado no tabular:** se intenta normalizar; un objeto suelto se envuelve como una fila; si está
anidado de verdad, **falla con un mensaje claro** que indique usar `recall_history`. Aplanar
automáticamente silencia el problema y produce columnas inventadas.

---

## Invariantes

1. **El original se persiste siempre, verbatim.** Nada de lo que se manda al modelo reemplaza a lo
   que se guarda.
2. **El mapa vive solo en el vector que arma el prompt.** El original sobrevive en tres consumidores
   y ninguno cambia:
   - el frame SSE `tool-output-available`, emitido en `agent_service.rs:654` — **antes** de que se
     construya el mensaje en la 658;
   - `all_tool_calls_executed`, que llega al `extra_info` del nodo;
   - `llm_node_history`.

   Esto significa que la UI de ADP no cambia, el operador ve el dato real al debuggear, y
   `recall_history` sigue siendo lossless.

   **Esta invariante depende de un orden entre dos líneas contiguas** y es exactamente el tipo de
   cosa que se rompe en silencio en un refactor. Va con test propio que falle si se invierten.
3. **Un error nunca se mapea.** `ToolResult` ya trae `success: bool` y `error`, así que es
   estructural: no se olfatea el texto.

---

## Observabilidad

Que se aplicó un mapa y qué se omitió tiene que verse en el stream, **como señal aparte y sin
alterar el frame del output**. El Item 14 del backlog ya lo pedía explícitamente.

---

## Estrategia de testing

| Qué | Cómo |
|---|---|
| El mapa | Golden tests — es determinista, así que el output se fija byte a byte |
| Degradación gradual | Tabla de tamaños contra filas crudas resultantes, incluyendo el caso "entra entero, sin encabezado" |
| Borde de interacción | Formas con `Some(vacío)`, `None`, rescue, suspend, y multi-mensaje de usuario sin respuesta |
| Borde con streaming | Test dedicado: es el único modo en que la regla puede mentir |
| Binding `from_tool_call` | Resolución OK, id inexistente, contenido no tabular, y **alcance cross-conversación que debe fallar** |
| Invariante de orden | Test que falle si el frame SSE pasa a emitirse después de construir el mensaje |
| E2E | Grafo real por el DAG engine: un agente con `data_run_python` que recibe un mapa y calcula sobre el original |

---

## Compatibilidad

- Un agente **sin** `data_run_python` se comporta exactamente como hoy: no hay mapa.
- El binding es aditivo; ninguna firma pública cambia.
- ADP no se entera: lo que cambia es el vector que arma el prompt, no el wire que ve la UI.

---

## Límites conocidos

- **No hay tope de tamaño**, ni acá ni en ningún lado del módulo LLM. Una interacción con muchos
  resultados grandes puede exceder la ventana del modelo y fallar contra el proveedor. Es una falla
  **ruidosa y atribuible**, no un resumen silencioso — elección deliberada.
- **La asignación es por resultado, no por interacción.** N resultados grandes en una misma
  interacción suman sin techo.
- El pinneo del primer mensaje de usuario (`messages[..SUMMARY_KEEP_FIRST_MSGS]`) **se cierra en este
  diseño** — ver [Zona 0](#zona-0--system). Queda como límite solo el tamaño del propio `system`,
  que va entero por definición: si las instrucciones de un agente son enormes, se pagan en cada
  turno. Eso es deliberado y no se toca.
- El mapa puede inducir al modelo a pedir el detalle más de lo necesario. Es medible: se instrumenta
  cuántos mapas son seguidos de una llamada a `data_run_python`.

---

## Diferido: capa 2 — extracción de prosa

Fuera de v1, **con el diseño escrito para no re-discutirlo**.

**Territorio real, más chico de lo que parece.** La mayoría de lo que llamamos prosa tiene
estructura arriba: una búsqueda web devuelve una *lista* de resultados con título, URL y cuerpo, y
eso **el mapa lo cubre**. Lo genuinamente no estructurado es un solo bloque opaco: un documento, una
respuesta larga en lenguaje natural.

**Si se implementa, tiene que extraer, no resumir.** El modelo barato se restringe a **seleccionar
pasajes**, nunca a generarlos: devuelve fragmentos copiados literales más la declaración de qué
salteó. Y se **verifica**: cada pasaje debe aparecer textual en la fuente, y el que no aparezca se
descarta. Es un chequeo determinista y barato que hace **imposible** la corrupción por
transcripción, dejando como único modo de falla "eligió los pasajes equivocados" — recuperable vía
`recall_history`.

**Por qué se difiere.** La ventana de ahorro es corta: un resultado grande vive en la zona 2 solo
hasta que la interacción cierra, y después se comprime igual en memoria, gratis. Así que el ahorro
no es "50k tokens para siempre" sino "50k por los turnos que le queden a esta interacción" — que
suelen ser uno o dos. Y el modelo barato lee el documento entero igual. Sumado: se paga una llamada
completa más latencia dentro del loop para ahorrar dos o tres turnos del modelo caro. A veces
conviene; no tenemos ningún dato para saber cuándo.

**Cómo se decide.** `COLMENA_DUMP_PROMPT_SIZES` ya existe. Se instrumenta qué fracción del gasto
real viene de bloques opacos de texto y se decide con números.

---

## Relación con otros trabajos

- **Item 14 del backlog** ("Output filtering para LLM"): este diseño cubre la mitad *reactiva* (qué
  hacer con lo que ya llegó). La mitad *proactiva* —proyección declarada por el operador o pedida
  por el modelo al llamar— sigue siendo Item 14 y es la capa de mayor ahorro y menor riesgo, porque
  el dato nunca entra a ningún prompt. Conviene atacarla primero o en paralelo.
- **El fix del panic de compactación** ([PR #174](https://github.com/Startti/colmena/pull/174),
  mergeado a `develop` como `1d6f807c` el 2026-08-22). **Este diseño lo deja obsoleto, y hay que
  decirlo explícito:** el clamp de `recent_boundary_by_tokens` protege el cálculo presupuestario del
  borde, y acá ese cálculo **deja de existir** — el borde pasa a ser estructural. Una vez que esto
  se implemente, `recent_boundary_by_tokens` se queda sin ningún llamador que decida el borde.

  **Qué hacer con la función es una tarea explícita de la implementación**, no algo que se resuelva
  solo: o se elimina junto con su barrido de invariante y sus tests de regresión, o se conserva con
  un llamador nuevo y documentado. Dejarla viva sin llamador es la peor de las tres opciones — deja
  código muerto que aparenta gobernar el borde. El PR #174 sigue siendo correcto y necesario
  mientras tanto: protege el comportamiento **de hoy**.

  Ese PR además **dejó documentadas dos limitaciones que este diseño cierra**, las dos consecuencia
  de que el borde lo decida solo el presupuesto (ver `docs/developer_guide/15_memory_guide.md`
  §Compactación → "Limitación conocida"):

  1. Cuando el guard de pares aterriza **por encima** de `keep_first`, un mensaje viejo de la
     interacción actual —típicamente la pregunta del `user` que disparó la tool— cae en la zona de
     memoria y se compacta a ~250 caracteres, mientras el resultado de la tool viaja verbatim al
     lado. El modelo recibe los datos y pierde el criterio para leerlos.
  2. Cuando aterriza **en `keep_first` o antes**, `build_compacted_messages` devuelve el historial
     entero sin compactar: no se pierde nada, pero tampoco se ahorra nada.

  Anclar el borde al último `assistant` sin `tool_calls` elimina las dos de raíz: ninguna parte de
  la interacción abierta puede caer en memoria, y la decisión deja de depender de dónde aterriza una
  caminata sobre mensajes `tool`.
- **El bug del `system` en Anthropic**: bloqueante para la zona 0. Trabajo aparte, ya iniciado.
