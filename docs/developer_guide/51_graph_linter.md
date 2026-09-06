# 51. Graph Linter — revisar un `graph.json` sin ejecutarlo

## El problema

El motor deserializa el `config` de cada nodo a un `serde_json::Value` sin tipar,
y ninguna struct del grafo usa `deny_unknown_fields`. La consecuencia es que
**un campo inventado no falla: se descarta en silencio**.

Escribir `"modle"` en vez de `"model"` produce un grafo que carga bien, pasa
`Graph::validate()`, arranca, y usa el modelo por defecto. El síntoma aparece
después, como comportamiento raro, nunca como un error de configuración.

El linter contesta la pregunta que realmente tiene quien escribe el JSON:
*¿cuáles de estos campos existen y cuáles me los inventé?*

## Uso

```bash
cargo run --bin dag_engine -- lint tests/graphs/basic/trigger.json
```

Salida sobre un grafo con problemas reales del repo:

```
Linting tests/graphs/edge_resolution/default_ports_chain.json
  error [UNKNOWN_NODE_PROPERTY] node "add_ten".default_input_port: "default_input_port" is not a property of a node; the engine discards it when loading the graph — move it into "config" if the node reads it there
  error [UNKNOWN_FIELD] node "add_ten".left: "left" is not a configuration field of add — the engine ignores it silently

  11 error(s), 0 warning(s), 0 info
```

Ese grafo estaba commiteado y falla al ejecutarse con
`Entrada no es un número: a`. El linter lo detecta **sin correrlo**.

Opciones:

| Flag | Efecto |
|---|---|
| `--format json` | Salida legible por máquina, con `code` estable por hallazgo |
| `--strict` | Sale con código ≠ 0 si hay algún error o warning |

El linter **no bloquea la ejecución**. `Graph::validate()` no cambió, y
`dag_engine run` se comporta exactamente igual que antes. Los grafos que hoy
corren en producción casi con seguridad contienen campos desconocidos, y
volverlos fail-closed rompería agentes en marcha sin aviso. `--strict` es el
camino de adopción para quien lo quiera en CI.

## Qué revisa

| Código | Severidad | Qué detecta |
|---|---|---|
| `UNKNOWN_NODE_TYPE` | error | El `type` del nodo no es un tipo que el motor sepa ejecutar |
| `UNKNOWN_FIELD` | error | **El campo inventado** — una clave del `config` que ese tipo de nodo no acepta, con sugerencia *did you mean*. También cubre los campos que el catálogo marca `read_only`: los popula el motor y escribirlos no hace nada (p. ej. `router.temperature`, fija en 0.1) |
| `UNKNOWN_NODE_PROPERTY` | error | Una clave del objeto nodo (junto a `type`/`config`) que el motor descarta al cargar |
| `MISSING_REQUIRED_FIELD` | error / warning | Un campo obligatorio que no está — ver más abajo |
| `INVALID_FIELD_VALUE` | warning | Un valor fuera del conjunto documentado |
| `FIELD_TYPE_MISMATCH` | warning | El tipo JSON no coincide con el documentado |
| `EDGE_UNKNOWN_NODE` | error | Un edge apunta a un nodo que el grafo no define |
| `DEAD_FIXED_CONFIG` | error | Una tool declara `fixed_config` junto a `node_schema`; el executor lee el segundo y **descarta el primero entero** |
| `REPURPOSED_TOOL_FIELD` | warning | Una tool —o el `target` de un `for_each`— fija una clave que el nodo destino no declara, en un tipo de nodo que **reinterpreta** las claves desconocidas en vez de ignorarlas |
| `TOOL_NEVER_EXPOSED` | error | Una tool sintética nombrada en `node_type` pero **no en la clave** de la entrada, que es lo que realmente la activa — el modelo nunca la recibe |
| `MALFORMED_TOOL_ENTRY` | error | Un `node_schema` embebido que no se puede parsear. En una entrada de tool **el motor rechaza el grafo al cargar**; en el `target` de un `for_each` el grafo arranca y el lote **muere fila por fila**. En los dos casos el linter lo dice antes de correr |
| `NO_CATALOG_COVERAGE` | info | El tipo de nodo no tiene entrada en el catálogo: **no se revisó** |

Los `code` son estables. Cualquier consumidor (la salida JSON, una UI sobre los
bindings) debe ramificar sobre ellos, nunca sobre el texto del mensaje.

## `DEAD_FIXED_CONFIG`: la regla de precedencia

Hay dos formas de configurar un nodo usado como tool, y **no se combinan**.
`DagToolExecutor` arma los argumentos de la llamada en un solo `if`/`else if`:

```rust
let inputs = if let Some(schema) = tool_cfg.and_then(|c| c.node_schema.as_ref()) {
    // PATH 0 (HIGHEST PRIORITY): node_schema
    merge_args_into_schema(&schema_value, args.clone())?
} else if let Some(fixed) = fixed_config.as_ref() {
    // ...
```

Con `node_schema` presente, el `fixed_config` **entero** se descarta — no sólo
las claves que colisionan. Escribir los dos es el anti-patrón que CLAUDE.md marca
como *WRONG — mixing*, y el linter ahora lo reporta como error nombrando cada
clave que se pierde:

```
error [DEAD_FIXED_CONFIG] node "agent".tool_configurations.http_upload.fixed_config:
  tool "http_upload" declares both "node_schema" and "fixed_config"; the executor
  reads "node_schema" and discards "fixed_config" entirely, so "url", "method",
  "headers" and "allow_http_urls" never reach the node
  — move each of them into "node_schema" as fixed fields, e.g. "url": { "fixed": … }
```

Ese ejemplo no es inventado: es el estado real de tres grafos de este repo antes
de la §20. El síntoma era `Invalid URL '': relative URL without a base`, y el
linter daba `no findings` porque el error estaba un nivel más abajo del `config`
del nodo.

Un `fixed_config` **vacío** junto a un `node_schema` no se reporta: descartar
nada no cuesta nada, y el autor no tiene ningún bug que arreglar.

**"Presente" no quiere decir "con campos".** `NodeSchema` es un `HashMap`, así
que `"node_schema": {}` deserializa a `Some(mapa vacío)`, el `if let Some(schema)`
matchea igual y el `fixed_config` se pierde exactamente como si el schema tuviera
campos. La regla mira presencia, no contenido — la primera versión exigía un
schema no vacío y era ciega justo al caso para el que existe. Lo encontraron dos
lenses de revisión, no las mutaciones: una mutación sólo ataca código que existe,
y ese test no existía.

El campo pasa por **dos** compuertas, no una. `Graph::validate()` corre en toda
entrada del motor ([`run_use_case.rs`](../../src/libs/colmena/src/dag_engine/application/run_use_case.rs))
y deserializa el `node_schema` crudo a `NodeSchema`, que es un `HashMap` **pelado**
—no un `Option`— así que rechaza el grafo antes de que ningún nodo corra:

| `node_schema` | `Graph::validate()` | Si pasa, el executor | El linter |
|---|---|---|---|
| ausente | pasa (no hay nada que validar) | lee el `fixed_config` — **el único caso vivo** | calla, correcto |
| `null` | **rechaza**: `malformed node_schema: invalid type: null` | nunca llega | calla |
| `{}` | pasa: un mapa vacío es un mapa válido | PATH 0; descarta el `fixed_config` | **`DEAD_FIXED_CONFIG`** |
| objeto poblado válido | pasa | PATH 0; descarta el `fixed_config` | **`DEAD_FIXED_CONFIG`** |
| objeto con un campo anidado inválido | **rechaza**: `NodeSchemaField` exige objeto | nunca llega | reporta igual — ver abajo |
| string, número, array, bool | **rechaza**: `invalid type` | nunca llega | calla |

Dos consecuencias que conviene tener presentes:

**Sólo la ausencia deja el `fixed_config` vivo.** Un `"node_schema": null` no es
equivalente a omitirlo: `tool_cfg.get("node_schema")` devuelve `Some(Value::Null)`
y el `HashMap` lo rechaza. El grafo entero falla al cargar.

**La regla sólo aporta valor en las dos filas que pasan la validación.** Ahí es
donde vivía el defecto de la §20. En las filas que `validate()` rechaza el linter
puede callar (inofensivo) o reportar (la fila del campo anidado inválido), pero en
ninguna de las dos importa: ese grafo no corre. Afinar la regla para distinguirlas
cambiaría un punto ciego por otro — una entrada malformada dejaría de disparar la
regla de precedencia — así que la respuesta correcta es un diagnóstico propio para
la entrada malformada, no un guard más astuto. Anotado en
[`BACKLOG.md`](../BACKLOG.md).

## Los campos de una tool

`DEAD_FIXED_CONFIG` revisa la *forma* de una entrada de tool. Esta regla revisa su
*contenido*: cada clave de `node_schema`, `fixed_config` y `node_config` se cruza
contra el `node_type` al que la tool apunta. Es el mismo chequeo que ya hace sobre
el `config` de un nodo, un nivel más abajo — y es el que faltaba cuando tres grafos
de este repo declararon `url` en `http_request`, cuyos campos son `base_url` y
`endpoint`.

**El set válido es `config_fields` + `input_ports` + `reserved_input_keys`.** Un
nodo despachado como tool recibe sus claves configuradas como **inputs**, no como
config. Medido sobre el corpus: revisar sólo contra `config_fields` reporta 16
grafos que funcionan (`task` en `subgraph`, `rows` y `user` en `python_script`).

**La severidad la decide lo que el nodo hace con una clave que no declara**, y eso
lo dice el propio catálogo con sus claves placeholder:

| El tipo de nodo | Placeholder | Diagnóstico | Cuántos nodos |
|---|---|---|---|
| Acepta cualquier clave como dato | `<any_key>`, `<any_text>` | nada | 5 |
| La **reinterpreta** | `<extra_keys>` | `REPURPOSED_TOOL_FIELD` (warning) | 1 |
| La ignora | ninguno | `UNKNOWN_FIELD` (error) | 31 |

Un placeholder que la regla **no** conozca cae a "acepta cualquier cosa": el
silencio es el veredicto seguro cuando el catálogo describe algo que el linter no
aprendió, porque la alternativa —tratar al nodo como contrato cerrado— reporta como
inventada cada clave que se le configure. Ese fallback tiene un costo invisible:
**una** clave placeholder nueva apagaría la regla entera para ese tipo de nodo sin
que nadie se entere. Por eso hay un test que recorre el catálogo y falla si aparece
un placeholder que la regla no maneje, nombrándolo. No es drift hipotético — el
catálogo ya usa otros cinco nombres (`<branch_name>`, `<child_output>`, `<raw>`,
`<raw_config>`, `<schema_fields>`), hoy confinados a `output_ports`, que esta regla
no lee.

La fila del medio es la que importa y la que costó entender. `http_request`
convierte **toda** clave no reservada en un query param
([`http.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs)),
así que `url` no se ignoraba en silencio: salía como `?url=https://…` contra una
URL base vacía. El mensaje lo dice tal cual, porque "campo inventado" habría sido
mentira:

```
warning [REPURPOSED_TOOL_FIELD] node "agent".tool_configurations.t.node_schema.url:
  "url" is not a field of "http_request"; that node type does not ignore an unknown
  key, it repurposes it — the value will be sent as a query parameter instead of
  configuring the node
```

Un `node_type` sin entrada en el catálogo produce **un** `NO_CATALOG_COVERAGE` por
entrada de tool, no uno por clave: los once `data_run_python` del corpus enterrarían
todo lo demás. Una entrada sin `node_type` no se toca — adivinar contra qué nodo
revisar sería peor que callar, y el motor la rechaza al cargar igual.

### Dos decisiones que no son obvias

**`input_ports` vive fuera de `NodeCatalogEntry`.** El cruce de la fase 2 compara
la entrada entera contra el `config_schema()` de cada nodo, y el alcance acordado
de esa declaración es el *config* del nodo. Meter los input ports adentro obligaría
a los 37 a declarar un segundo eje sin ganancia a nivel de nodo, así que el catálogo
los guarda en un mapa lateral.

**Se revisa también `node_config`.** Es el bloque que usan las entradas toolkit
(`expose_sub_tools`): las 15 del corpus configuran su nodo por ahí y nunca por
`fixed_config`.

## El `target` de un `for_each`

Un `for_each` embebe `{node_type, node_schema}` —**la misma forma que una entrada de
tool**— y lo despacha una vez por fila. Ese bloque nombra campos de otro tipo de nodo,
así que se revisa con la regla de arriba, la misma pregunta y la misma severidad:
*¿ese tipo de nodo lee esta clave?*

```
warning [REPURPOSED_TOOL_FIELD] node "loop".target.node_schema.url:
  "url" is not a field of "http_request"; that node type does not ignore an unknown
  key, it repurposes it — the value will be sent as a query parameter instead of
  configuring the node
```

Ese es el defecto de la §20 un contenedor más abajo, y hasta acá no lo veía nadie.

**Tres puertas, no una.** El nodo resuelve su `target` con `cfg_or_input`, así que el
bloque llega por el `config` del propio nodo, o —cuando el `for_each` está expuesto
como tool— por `node_schema.target.fixed` o por `fixed_config.target` de la entrada.
Las tres se recorren. Lo que el modelo elige en tiempo de ejecución no está acá para
ser leído.

### Un `node_schema` roto en un `target` falla tarde, no al cargar

Y por eso su mensaje **no puede ser el de una entrada de tool**. Un `node_schema`
malformado dentro de `tool_configurations` hace que `Graph::validate()` rechace el
grafo **al cargar**; dentro de un `target` no, porque `validate()` sólo mira
`config.tool_configurations`. El grafo arranca.

Lo que pasa después está **medido**: cada fila falla al despachar con
`Invalid node_schema: …`, porque `merge_args_into_schema` corre las mismas dos
comprobaciones antes de que la fila vaya a ningún lado. El lote muere a mitad de la
corrida en vez de antes — que es justamente el viaje que ahorra un lint.

| Grafo corrido | Resultado |
|---|---|
| Control: schema **válido**, requerido que ninguna fila aporta | `err=2`, `missing required param 'must_be_present'` |
| Familia 1: `"body": "not-an-object"` | `err=2`, `Invalid node_schema: invalid type: string` |
| Familia 2: campo visible al LLM sin `type` | `err=2`, `Invalid node_schema: … is LLM-visible but missing type` |

En los tres, cero requests salieron al servicio. Y sobre los mismos dos grafos el
linter reporta `MALFORMED_TOOL_ENTRY` **sin correr nada**.

> **Una afirmación anterior era falsa.** Decía que las filas despachaban **sin
> validar**, razonando desde el par `if let Ok(...)` sin rama else de `for_each.rs`.
> Correrlo lo desmintió: el merge rechaza la fila primero, así que ese bloque es
> inalcanzable para un schema malformado. Mandaba a un operador a buscar filas
> corruptas que no existen.

**Las claves se revisan igual aunque el schema esté roto.** Son defectos
independientes: que una clave no sea campo de ese tipo de nodo sigue siendo cierto y
accionable cualquiera sea la forma del bloque.

## Los cinco tipos que sólo existen dentro de `tool_configurations`

`node_types` en el catálogo está **cerrado en las dos direcciones** contra el
registry del motor: un test falla si documenta algo que el motor no ejecuta, y
otro si el motor registra algo sin documentar. Pero cinco nombres son válidos
como `node_type` de una tool y **no** son nodos registrados — cuatro tools
sintéticas que `llm_call` ensambla, más `mcp`, que es un servidor remoto. Viven
en una sección aparte, `tool_only_node_types`.

Sin esa lista el linter no podía distinguir `data_run_python` —correcto, y usado
por once grafos de este repo— de `data_run_pythonn`, que no expone nada. Decía lo
mismo de los dos.

### La trampa: para cuatro de ellos, `node_type` es inerte

Una entrada de `tool_configurations` tiene dos identificadores, la **clave** del
mapa y el campo **`node_type`**. Para un nodo normal manda el `node_type`, y la
clave es sólo el nombre que ve el modelo. Para estas cuatro es al revés:
`llm_call` junta las claves en `configured_aliases` y pregunta
`configured_aliases.contains("data_run_python")`. El `node_type` de esa entrada
no lo lee nadie.

```json
"mi_python":       { "node_type": "data_run_python" }    // no expone NADA
"data_run_python": { "node_type": "lo_que_sea" }         // sí expone la tool
```

Nada avisa. `available_tools` sí busca el nombre en el registry, obtiene `None`
y descarta la entrada **sin rama `else` y sin log** — la rama de toolkits de al
lado sí advierte, ésta no. El grafo carga, valida, corre y termina en cero; el
único síntoma es un agente que dice que no puede hacer la tarea.

Verificado corriendo dos grafos idénticos salvo la clave: el keyeado
`data_run_python` emitió frames `tool-input-*` y `tool-output-available` y el
modelo llamó la tool; el keyeado `mi_python` no emitió ninguna frame de tool y
el agente contestó que no tenía herramienta. Los dos salieron con código 0.

`TOOL_NEVER_EXPOSED` reporta ese caso. La regla lee de qué forma se activa cada
tipo desde el catálogo (`activated_by`), así que `mcp` —donde el `node_type`
**sí** selecciona la entrada— queda fuera por construcción y no por una
excepción escrita a mano.

**Por qué esta regla no podía ir en otra rebanada.** Enseñarle al linter estos
cinco nombres significa **saltearlos**, y saltearlos sin la regla convierte el
caso de arriba de un `NO_CATALOG_COVERAGE` débil a silencio completo — peor que
antes de que el catálogo conociera los nombres. Lo detectó una revisión y la
respuesta fue publicar las dos cosas juntas. Un test lo fija:
`teaching_the_linter_these_names_never_makes_a_broken_entry_quieter`.

El **comportamiento** de estas cuatro se configura sólo por `fixed_config`, que
lee código propio de cada tool; `node_schema` y `node_config` no le llegan. Por
eso el catálogo documenta su existencia y su forma de activación
(`activated_by`), pero no sus campos — el linter no podría validarlos.

Cuidado con leer eso como "esos bloques son inertes". No lo son: bajo **lazy tool
loading** —lo que corre en producción— la entrada igual entra al catálogo de
`describe_tool`, que lee `name`, `description` y `summary`, y renderiza el
`node_schema` como la tabla de parámetros que ve el modelo. `enters_lazy_catalog`
sólo excluye las entradas `eager` y las `mcp`.

## Decirlo antes de correr, no después

`Graph::validate()` rechaza el grafo entero cuando el `node_schema` de una tool no
se puede leer, y desde el §18 esa validación corre en **toda** entrada del motor,
no sólo desde el CLI. Así que el diagnóstico ya existía; lo que faltaba era decirlo
**antes** de ejecutar, que es la razón de ser del linter.

Son dos familias de rechazo, no una, y hacen falta las dos para cubrir el caso:

1. **El bloque no deserializa a `NodeSchema`** — `null`, un escalar, un array, o
   un objeto alguna de cuyas entradas no es una definición de campo válida.
2. **Deserializa pero `parse_node_schema` lo rechaza** — un campo visible al LLM
   sin `type`, o uno de tipo `array` sin `items` / `items.type`. Ojo con esta
   familia: `{ "body": { "required": true } }` es un `NodeSchemaField` perfectamente
   bien formado, así que la primera familia no lo ve, y el motor igual lo rechaza.

`MALFORMED_TOOL_ENTRY` lo dice, y lo dice **llamando a las mismas funciones** que
usa `validate()` —deserializar a `NodeSchema` y después `parse_node_schema`— en vez
de reimplementar la regla. Las dos viven en el dominio, igual que el linter, así
que compartirlas no cruza ninguna frontera y hace la divergencia imposible por
construcción.

Verificado forma por forma contra el binario: las nueve que el motor rechaza —seis
de la primera familia, tres de la segunda— las reporta el linter, y las dos que
acepta —`node_schema` ausente y uno bien formado— no. La tabla de casos del test
las enumera una por una.

**El mensaje nombra formas y claves, nunca un valor.** La versión original
reenviaba el error de serde tal cual, y serde imprime el string ofensor
literalmente. Es decir que la forma más probable de cometer este error —poner un
valor directo bajo la clave en vez de dentro de una definición de campo— copiaba
ese valor a stdout y al reporte `--format json`, que se lee en logs de CI donde el
cuerpo del grafo no aparece. Un `"node_schema": { "api_key": "sk-live-…" }` filtraba
la credencial. Ahora dice `` `api_key` es un string ``, con las claves ofensoras
ordenadas alfabéticamente: así la oración no depende del orden en que el autor
escribió las claves. Los errores de `parse_node_schema` se reenvían sin tocar,
porque nombran la etiqueta del campo y nunca su valor.

Con una excepción que conviene nombrar en vez de fingir que no existe:
`INVALID_FIELD_VALUE` **sí** imprime un valor, al lado de la lista de aceptados.
Pero sólo para un campo que el catálogo declara con `valid_values` —un enum
cerrado como `method`—, y ahí no aterriza una credencial. Una clave de
`node_schema` es lo contrario: un lugar libre que nombra el autor.

**Y una segunda fuente de inestabilidad, en la otra rama.** `parse_node_schema`
recorre un `HashMap` y devuelve el **primer** campo que le disgusta, así que con
dos campos malos el motivo que imprime depende del seed de hash del proceso. Para
un crash de carga da igual; para un reporte que se diffea en CI, no. El linter le
pregunta de a un campo por vez en orden alfabético y se queda con la primera queja
—misma función, mismo veredicto, oración estable—, y al final le pasa el esquema
entero igual, como **guarda contra deriva**: hoy no atrapa nada, porque todos los
errores de esa función viven en su bucle por campo y su segunda pasada —la que
renombra hijos de contenedor que colisionan— no tiene rama de error. Verificado
contra el binario con dos contenedores compartiendo una clave hija: sin hallazgo.
Cuesta una pasada y sirve para el día en que esa función crezca un error realmente
entre campos.

**Reportar esto silencia la regla de precedencia para esa entrada, y sólo esa.**
El consejo de `DEAD_FIXED_CONFIG` —mover las claves *dentro* de ese mismo
`node_schema`— dejaría el grafo igual de inejecutable, porque el schema es
justamente lo que está roto. Antes de esta regla, un `node_schema` con un campo
anidado inválido recibía exactamente ese consejo.

Las **reglas de campos no se silencian**. Son defectos independientes: "esta clave
no es un campo de ese tipo de nodo" sigue siendo verdad y sigue siendo accionable
cualquiera sea la forma del `node_schema`, y esconderla costaría un viaje de ida y
vuelta extra por algo que no tiene relación con el primer error. Una entrada
malformada *y* con una clave inventada en `fixed_config` reporta las dos cosas.

### Y un tipo tool-only usado como nodo del grafo

Es un nombre exacto en el lugar equivocado, no un hueco de cobertura. Antes se
reportaba como `NO_CATALOG_COVERAGE` (info) aconsejando agregar una entrada al
catálogo — **imposible de seguir**, porque `node_types` está cerrado en ambas
direcciones contra el registry y esa entrada haría fallar el test suite. Ahora el
consejo dice dónde va el nombre. Un tipo realmente desconocido conserva el consejo
original.

La **severidad depende del contexto**, y conviene no confundirlas. Con
`LintContext::from_catalog` —lo que usa el CLI— pasa a ser un `UNKNOWN_NODE_TYPE`
(error, porque ese grafo no corre). Con `with_embedded_catalog` sigue siendo
`NO_CATALOG_COVERAGE` (info): ese contexto no saca conclusiones sobre tipos de
nodo, así que sólo cambia el texto del consejo.

## Catálogo de ejemplos

Cada archivo de [`tests/lint_examples/`](../../tests/lint_examples) está roto a
propósito y demuestra **una** cosa. La salida de abajo no está transcrita a mano: es lo
que imprime el binario, y
[`tests/lint_examples.rs`](../../src/libs/colmena/tests/lint_examples.rs) falla si
alguno deja de producir el diagnóstico que acá se muestra.

Viven **fuera** de `tests/graphs/` a propósito: ese árbol es el corpus sobre el que se
mide el ruido del linter, y archivos rotos a propósito envenenarían justo el número que
dice si la herramienta vale la pena escuchar.

Esta primera tanda cubre los diagnósticos **a nivel de nodo**. Los de
`tool_configurations`, los del `target` de un `for_each` y el punto ciego del `subgraph`
inline llegan en la tanda siguiente, junto con el test que exige que ningún
`DiagnosticCode` se quede sin ejemplo.

Para correr cualquiera:

```bash
cargo run --bin dag_engine -- lint tests/lint_examples/01_invented_config_field.json
```

### El campo inventado

[`01_invented_config_field.json`](../../tests/lint_examples/01_invented_config_field.json) — `modle` está a una edición de `model`. El grafo carga, corre, y usa el modelo por defecto — el síntoma aparece después y no como error de configuración.

```
  error [UNKNOWN_FIELD] node "chat".modle: "modle" is not a configuration field of llm_call — did you mean "model"?

  1 error(s), 0 warning(s), 0 info
```

### Un tipo de nodo que no existe

[`02_unknown_node_type.json`](../../tests/lint_examples/02_unknown_node_type.json) — Un near-miss contra un tipo documentado es evidencia fuerte de typo, así que es error y no info.

```
  error [UNKNOWN_NODE_TYPE] node "chat": "llm_cal" is not a documented node type — did you mean "llm_call"?

  1 error(s), 0 warning(s), 0 info
```

### Una clave del objeto nodo

[`03_unknown_node_property.json`](../../tests/lint_examples/03_unknown_node_property.json) — Va al lado de `type`/`config`, no adentro. El motor la descarta al cargar y nadie avisa.

```
  error [UNKNOWN_NODE_PROPERTY] node "run".default_input_port: "default_input_port" is not a property of a node; the engine discards it when loading the graph — move it into "config" if the node reads it there

  1 error(s), 0 warning(s), 0 info
```

### Un campo obligatorio que falta

[`04_missing_required_field.json`](../../tests/lint_examples/04_missing_required_field.json) — Ningún edge entrante puede aportarlo, así que es error. Con un edge sin nombre de puerto sería warning.

```
  error [MISSING_REQUIRED_FIELD] node "chat".api_key: required field "api_key" is not set, and no incoming edge supplies it

  1 error(s), 0 warning(s), 0 info
```

### Un valor fuera del conjunto documentado

[`05_invalid_field_value.json`](../../tests/lint_examples/05_invalid_field_value.json) — El linter enumera los aceptados.

```
  warning [INVALID_FIELD_VALUE] node "call".method: "PATCHH" is not one of the documented values for "method" — accepted: "GET", "POST", "PUT", "DELETE", "PATCH"

  0 error(s), 1 warning(s), 0 info
```

### El tipo JSON no coincide

[`06_field_type_mismatch.json`](../../tests/lint_examples/06_field_type_mismatch.json) — Warning y no error: el catálogo tiene tipos en prosa (`any`, uniones), y equivocarse acá cuesta más que callarse.

```
  warning [FIELD_TYPE_MISMATCH] node "chat".temperature: "temperature" is documented as number but the value is a string

  0 error(s), 1 warning(s), 0 info
```

### Un edge que no apunta a nada

[`07_edge_to_nowhere.json`](../../tests/lint_examples/07_edge_to_nowhere.json) — En ejecución resuelve a `null` y el grafo *parece* funcionar. Nombrarlo es todo el punto.

```
  error [EDGE_UNKNOWN_NODE]: edge to="log_result" names a node that this graph does not define

  1 error(s), 0 warning(s), 0 info
```

### Sin cobertura no se opina

[`08_no_catalog_coverage.json`](../../tests/lint_examples/08_no_catalog_coverage.json) — No se parece a nada documentado, así que el linter dice que no puede revisarlo en vez de marcar cada campo como inventado.

```
  info [NO_CATALOG_COVERAGE] node "n": "quantum_flux_capacitor" has no entry in the node catalog, so this node's configuration was not checked — if the engine registers it, add an entry to docs/node_configurations.json to enable checking

  0 error(s), 0 warning(s), 1 info
```

### Un campo que popula el motor

[`09_read_only_field.json`](../../tests/lint_examples/09_read_only_field.json) — `router.temperature` está fija en 0.1 en los dos modos del nodo. Escribirla no hace nada.

```
  error [UNKNOWN_FIELD] node "route".temperature: "temperature" is populated by the engine on router and cannot be set here — remove it; the value written here has no effect

  1 error(s), 0 warning(s), 0 info
```

### El control

[`18_clean_graph.json`](../../tests/lint_examples/18_clean_graph.json) — Un grafo correcto no produce absolutamente nada. Sin esto, el resto del catálogo no prueba que el linter distinga.

Y no alcanza con que el linter calle: **este control además corre**. Pasado por el motor
contra OpenAI termina en `[DONE]` sin errores, con `promptTokens: 842` y
`completionTokens: 20`. Un control que lintea limpio pero no arranca no serviría de
control — diría que el linter calla, no que tiene razón.

```
  no findings
```

## Las decisiones que evitan el ruido

Un linter con falsos positivos se ignora. Cinco reglas existen sólo para eso, y
cada una salió de medir contra los grafos de ejemplo del repo.

**1. Sin cobertura no se opina.** Un tipo de nodo sin entrada en el catálogo
produce un `NO_CATALOG_COVERAGE` (info) y **ni un solo** `UNKNOWN_FIELD`. Marcar
como inventados los campos de un nodo que no sabemos leer sería la forma más
rápida de perder la confianza de todos.

**2. `required` no significa "tiene que estar en `config`".** Varios nodos
resuelven un campo obligatorio desde un edge entrante (patrón `cfg_or_input`).
El edge **nombra el puerto** al que escribe (`"to": "run_sql.query"`), así que
si un edge nombra el campo faltante, no falta: no se reporta nada. Si el nodo
sólo tiene edges sin nombre de puerto (`"to": "run_sql"`, que apunta al puerto de
entrada por defecto), baja a warning y lo dice. Sólo es error cuando ningún edge
puede aportarlo. Mirar únicamente *"¿tiene algún edge entrante?"*, ignorando el
nombre del puerto, tiraba una respuesta que el grafo ya daba: producía 35 de 41
avisos falsos.

Además, una obligatoriedad **condicional** — el catálogo la expresa en prosa,
como `router.schema` ("mode B only") — nunca se reporta: es una condición que el
linter no puede evaluar, y adivinarla da errores sobre grafos correctos.

**3. Hay nodos con config abierta.** `input` y `mock_input` emiten su propia
config como datos para los nodos siguientes, así que cualquier clave es la forma
prevista de usarlos, no una invención. El catálogo lo declara con una clave
placeholder entre ángulos, `<any_key>`. Sin esta regla, esos dos tipos solos
generaban 93 de 178 hallazgos.

**4. Un comentario no es un ajuste.** Claves de anotación (`comment`,
`_comment`, `$comment`, `description`, y cualquiera que empiece con `_` o `$`) se
ignoran, en la raíz del grafo y dentro de `config` — salvo que el tipo de nodo
documente un campo con ese nombre, en cuyo caso se revisa normalmente. Los grafos
del repo tienen más de 260 anotaciones. Las claves reservadas del motor
(`__colmena_*`) tampoco se tocan.

**5. Hay claves de config que lee el motor, no el nodo.** `include_extra_info` la
lee `DagRunUseCase` del `config` de **cualquier** nodo al armar la salida final,
así que no pertenece al `config_fields` de ningún tipo. El catálogo las declara
aparte, en `common_config_fields`. Sin esa sección el linter marcaba como
inventada una clave que funciona — y afirmaba, falsamente, que el motor la
ignora: 24 de los 79 `UNKNOWN_FIELD` del corpus.

## Por qué hay dos puntos de entrada

```rust
lint_graph(&graph, &ctx)                 // desde un Graph ya deserializado
lint_graph_json(&document, &ctx)         // desde el JSON crudo
```

**Preferí siempre `lint_graph_json` cuando tengas el documento original.**
Deserializar a `Graph` descarta en silencio toda clave no declarada, así que un
nodo con `"default_input_port"` — una invención real presente en los grafos de
ejemplo de este repo — **ya no existe** cuando hay un `Graph`. Sólo el documento
crudo puede reportarla.

`lint_graph` sigue existiendo para quien ya tiene un `Graph` en la mano, como el
worker de ADP, que recibe el grafo por Redis.

## De dónde sale la autoridad

El linter puede juzgar un tipo de nodo con dos grados de certeza distintos, y
`KnownNodeTypes` los separa a propósito, porque deciden qué le está permitido
**afirmar**:

| Variante | Qué sabe | Qué reporta ante un tipo desconocido |
|---|---|---|
| `Registry(&set)` | El registry real del motor | `UNKNOWN_NODE_TYPE` (error): *"is not a node type this engine can run"*. La ausencia es prueba. |
| `CatalogOnly` | Solo los tipos documentados | Si es un tipo tool-only → `UNKNOWN_NODE_TYPE` (error): el nombre va dentro de `tool_configurations`. Si hay un tipo documentado a distancia de typo → `UNKNOWN_NODE_TYPE` (error), *"is not a documented node type"*. Si no → `NO_CATALOG_COVERAGE` (info): no puedo revisarlo. |
| `Unchecked` | Nada | No opina sobre tipos de nodo. |

La CLI usa `CatalogOnly`, que no cuesta nada construir —sin engine, sin base de
datos— y es lo que permite lintear un archivo JSON suelto. Lo que resigna es la
autoridad para decir que un tipo no se puede ejecutar: **decir eso con solo el
catálogo en mano es falso para un nodo que sí está registrado pero todavía no
documentado**, que es justamente la forma más probable de que aparezca un tipo
desconocido. Un near-miss contra un tipo documentado sí es evidencia fuerte de
typo, y se reporta como error.

Si tenés un registry a mano, `LintContext::from_registry(catalog, &tipos)`
recupera la afirmación fuerte.

## De dónde sale la verdad

El catálogo de campos por nodo es
[`docs/node_configurations.json`](../node_configurations.json), embebido en el
binario con `include_str!`. Se embebe **ese mismo archivo**, no una copia: el
documento que leen las personas y los agentes y el que aplica el linter son los
mismos bytes, así que no pueden discrepar.

Tres tests lo sostienen:

- `declared_node_types_all_have_an_entry` — el archivo no puede contradecirse a
  sí mismo. Hasta ahora sí lo hacía: declaraba 37 tipos válidos y sólo
  documentaba 32.
- `every_registered_node_type_is_documented_in_the_catalog` — todo tipo que el
  registry sabe ejecutar tiene entrada.
- `the_catalog_documents_no_node_type_the_engine_cannot_run` — y a la inversa.

Los dos últimos construyen el registry **con todas sus dependencias opcionales**:
`secure_suspend` sólo se registra si hay `SecureValueService`, y
`image_generation` / `image_edit` / `tts` sólo si hay adapter de storage. Un
registry armado sin ellas encogería el conjunto bajo prueba en silencio.

> **Nota de empaquetado.** `docs/` queda fuera del package root del crate
> (`src/libs/colmena/`), así que `cargo package` no podría resolver el
> `include_str!`. Hoy no es una restricción — el crate se consume como
> dependencia git, nunca desde un registry — y el precedente ya existe en
> [`log_policy.rs`](../../src/libs/colmena/src/dag_engine/log_policy.rs). Si eso
> cambiara, el camino es generar un artefacto dentro del crate.

## Ruido medido

Sobre los grafos de ejemplo del repo el linter produce **80 hallazgos**. Cada
categoría fue auditada contra el código: no hay falsos positivos conocidos. Si
agregás una regla, medí de nuevo — la primera versión de este linter producía 178
hallazgos de los que 132 eran ruido, y sin medir eso no se nota.

`DEAD_FIXED_CONFIG` se midió antes de escribirla: sobre las 206 entradas de
`tool_configurations` del corpus dispara **cero** veces, porque los únicos tres
casos que existían se corrigieron en la §20. Una regla que no dispara hoy no es
una regla inútil: contra el grafo roto de entonces dispara y nombra las cuatro
claves que se perdían.

## Limitaciones conocidas

- **El catálogo se mantiene a mano, y la fase 2 lo está cerrando nodo por nodo.**
  Un nodo puede declarar sus campos en código vía `ExecutableNode::config_schema()`
  (solo los hechos mecánicos: nombres, `required`, `valid_values`, `read_only`); un
  test exige que esa declaración coincida con el catálogo, así que para un nodo
  migrado los dos ya no pueden diverger. La prosa (`description`/`example`/`default`)
  sigue viviendo en el JSON a propósito — es lo que leen humanos y agentes. Los
  nodos que todavía devuelven `None` siguen respaldados solo por el catálogo.
- **De las cinco compuertas de `Graph::validate()`, el linter reproduce una.** Esta
  es la limitación que más conviene tener presente, porque decide qué significa un
  reporte limpio. `MALFORMED_TOOL_ENTRY` reproduce el brazo `node_schema`. Las otras
  cuatro rechazan el grafo al cargar y el linter **no dice nada**: un `memory_mode`
  con un valor que no es del enum, uno sobre un tipo de nodo que no lleva memoria,
  uno que lleva memoria sin `connection_url`, y un bloque `mcp` malformado o con URL
  no-HTTPS. Hay una sexta fuera de las entradas de tool —un node id que contiene
  `/`— que tampoco se revisa.

  **Qué significa entonces "no findings":** que el linter no encontró nada de lo que
  sabe buscar, no que el motor vaya a cargar el grafo. El motor sigue siendo la
  autoridad y falla cerrado en los cinco casos, así que lo que se pierde es el aviso
  temprano, nunca una ejecución sin guardia. Los items abiertos están en el
  [BACKLOG](../BACKLOG.md) como L1 y L1b.

  *(Esta viñeta decía hasta la §22 que los campos de una tool no se cruzaban contra
  su `node_type`. Eso se cerró en esa misma sección y la limitación quedó vieja
  contradiciendo a "Los campos de una tool" más arriba.)*
- **Ninguna regla entra en un `subgraph` inline.** El `target` de un `for_each` ya se
  revisa (ver arriba), pero el `child_graph_inline` de un `subgraph` sigue siendo un
  `Value` opaco para el linter: los nodos que viven ahí adentro no se revisan, con
  campos inventados y edges colgados incluidos. Lo que amortigua el caso es que un
  hijo inline malformado igual falla al cargar, porque `validate()` sí corre para los
  grafos hijos — se pierde el aviso temprano, no la guardia. Abierto en el
  [BACKLOG](../BACKLOG.md) como L2.
- **Cuatro tipos condicionales se dan por disponibles.** El linter revisa el
  grafo contra lo que el motor *puede* ejecutar, no contra el cableado de un
  despliegue concreto.
- **Las claves de la raíz del grafo no se revisan.** En los 301 grafos del repo,
  todas las claves raíz no declaradas son anotaciones; marcarlas enterraría los
  hallazgos que importan.
- **Sin registry, el linter no afirma qué puede ejecutar el motor.** Es una
  limitación deliberada, no un hueco: ver "De dónde sale la autoridad" abajo.
