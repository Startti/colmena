# Cambios recientes — 2026-09

> **Alcance:** Commits sobre `develop` desde el cierre de `2026-08`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. `sql_query`: eliminado el flag fantasma `guardrail_enabled`

**Qué cambió.** El nodo `sql_query` anunciaba en su `schema()` un campo de config
`guardrail_enabled` ("enables static validation rules") que **ningún código leía
jamás**. No existía ni existió nunca un `config.get("guardrail_enabled")` en
`sql.rs`. Un operador que ponía `guardrail_enabled: false` esperando desactivar la
validación estática no obtenía ningún efecto, y tampoco ninguna advertencia.

El campo se **eliminó** en lugar de cablearse. La validación estática es lo que
bloquea `DROP`, `TRUNCATE` y `DELETE`/`UPDATE` sin `WHERE`: hacerla apagable
habría sido un downgrade de seguridad, no un arreglo. Ahora es explícitamente
incondicional, con una nota en `sql.rs` para que el flag no se reintroduzca.

`guardrail_llm` **no se tocó** — ese guardrail sí es real y sigue siendo opcional
(`guardrail_llm.enabled`, default `false`).

**Sin cambio de comportamiento.** El campo vivía en el bloque `config` del
`schema()`, que es puramente descriptivo: el motor solo consume el bloque `inputs`
para construir la tool definition del LLM, y nunca valida claves desconocidas.
Por eso:

- Los grafos persistidos (incluidos los de ADP) que aún pasan
  `guardrail_enabled` como campo `fixed` **siguen funcionando sin cambios** — la
  clave sobrante se ignora, exactamente igual que antes.
- No hay cambio de API pública → **ADP no afectado**.

Se limpiaron además los lugares que propagaban el campo: la guía 23, el
`node_configurations.json` canónico, cuatro grafos de `tests/graphs/agents/` y la
skill `capability-data-sql`, que le enseñaba a los operadores a declararlo.

**Verificación.**

| Chequeo | Resultado |
|---|---|
| `cargo test --lib sql` | 182 passed, 0 failed |
| `cargo test --lib static_validator` | 27 passed, 0 failed — los bloqueos siguen intactos |
| E2E real vía DAG engine (`sql_query_readonly_test.json`, OpenAI + Postgres) | exit 0, el tool consultó la BD y devolvió tablas y `row_count` reales |

**Documentación de referencia.**
- [`docs/developer_guide/23_sql_node.md`](developer_guide/23_sql_node.md) — tabla de configuración.
- [`docs/qa/nodes/sql_query.md`](qa/nodes/sql_query.md) — hallazgo A1, marcado como resuelto.
- [`docs/qa/nodes/RESUMEN_GAPS.md`](qa/nodes/RESUMEN_GAPS.md) — resumen priorizado del audit.

**Origen.** Hallazgo de severidad Alta A1 del audit doc-vs-código por nodo (PR #226).

**Estado.** done.

---

## 2. Loop de grafo: guardia contra ejecución sin fin

**Qué cambió.** Un `loop_status` mal escrito podía dejar un loop de serve-mode
girando indefinidamente. `loop_controller` propagaba el valor tal cual, y el único
consumidor real (`api.rs`) solo detiene el loop cuando lee exactamente
`"FINISHED"` (o una suspensión, o un nodo de output). Un `"FINISHEDD"` no coincide
con nada, así que el motor tomaba otro turno. Para siempre.

**Los límites por nodo no cubrían este caso.** `max_total_calls` y
`max_calls_from` viven dentro de `RunUseCase`, y cada turno del loop es un
`run_dag` nuevo: sus contadores se reconstruyen desde cero en cada iteración. El
`turn_count` de `api.rs` existía, pero solo se imprimía — nunca se comparaba
contra nada. (`COLMENA_HARD_TURN_CAP` es de otra capa: acota los turnos del
agente LLM dentro de `AgentService`, no las iteraciones del grafo.)

Dos cambios, en dos capas distintas:

1. **`loop_controller` coacciona los valores desconocidos.** Valida contra
   `KNOWN_LOOP_STATUSES` = `NEXT_TURN`, `FINISHED`, `SUSPENDED`, `FINISHED_PHASE`,
   y convierte cualquier otro valor a `FINISHED` emitiendo un `warn`. Parar
   temprano es un fallo visible y depurable; un loop sin fin no lo es.

2. **Techo de turnos en `api.rs`** (`COLMENA_MAX_GRAPH_TURNS`, default `50`,
   `0` = sin techo), aplicado a los **dos** loops — el de JSON y el de streaming.
   Ataca la causa raíz: protege también cuando el runaway no viene de un typo
   (un orquestador que nunca emite `FINISHED`, un grafo sin nodo de output).

**Por qué NO se hizo fail-closed estricto.** Era la opción obvia y es la
equivocada: el enum documentado estaba **incompleto**. `orchestrator.rs:585` emite
`FINISHED_PHASE`, que no aparecía en `valid_values`. Rechazar los valores fuera de
la lista habría roto el orquestador en producción. Por eso `FINISHED_PHASE` es
ahora un valor válido de primera clase, con un test que verifica explícitamente
que **no** se colapsa a `FINISHED` (colapsarlo cortaría el loop una fase antes).

**Al alcanzar el techo la ejecución falla de forma ruidosa,** nunca devuelve la
última salida parcial como si el grafo hubiera terminado bien:

- **JSON**: HTTP 500 con `{ error, turns, last_output }`.
- **SSE**: un frame `{"type":"error","error":"Loop stopped after N turns..."}`.

**Compatibilidad.** Aditivo. Los cuatro estados válidos se comportan igual que
antes; solo cambian los valores que ya estaban rotos. El techo por defecto (50)
solo afecta a peticiones `?loop=true` que hoy no terminan — es decir, a las que ya
estaban colgadas. Sin cambio de API pública → **ADP no afectado**.

**Verificación.**

| Chequeo | Resultado |
|---|---|
| `cargo test --lib loop_controller` | 6 passed, 0 failed |
| Prueba de mutación (corrección desactivada a propósito) | `unrecognized_status_is_coerced_to_finished` **falla** — el test detecta el defecto real, no pasa por construcción |
| `cargo test --verbose` | ver PR |

**Documentación de referencia.**
- [`docs/developer_guide/12_dag_engine_guide.md`](developer_guide/12_dag_engine_guide.md) — "Techo de turnos del loop".
- [`docs/node_configurations.json`](node_configurations.json) — `loop_controller.loop_status`, con `FINISHED_PHASE` y la coerción.
- [`docs/agent_context/node_ports_reference.md`](agent_context/node_ports_reference.md) — puertos y salida del nodo.
- [`docs/qa/nodes/loop_controller.md`](qa/nodes/loop_controller.md) — hallazgo A2, marcado como resuelto.

**Origen.** Hallazgo de severidad Alta A2 del audit doc-vs-código por nodo (PR #226).

**Estado.** done.

---

## 3. Catálogo de nodos: cerrados los huecos y la contradicción interna

**Qué cambió.** `docs/node_configurations.json` describía **32** tipos de nodo
mientras declaraba **37** como válidos en `common_node_properties.type.valid_values`
y los referenciaba en `categories`. Faltaban las entradas de `tavily_client`,
`api_explorer`, `image_generation`, `image_edit` y `tts`. Nada detectaba esa
contradicción: el archivo se mantenía a mano, sin generador ni check en CI.

Las cinco entradas ahora existen, auditadas campo por campo contra la
implementación de cada nodo. Además:

- **Clave `required` duplicada** en `llm_call.crdt_documents`: el objeto tenía dos
  (`false` del campo, y una lista `["artifact_id"]` estilo JSON-Schema mal
  ubicada). `jq` la absorbía en silencio con last-wins; un parser tipado la
  rechaza. Se eliminó la segunda, redundante con `properties.artifact_id.required`.
- **Campos que el código lee y el catálogo no documentaba**:
  `llm_call.max_tool_result_bytes`, `orchestrator.api_key` y `orchestrator.plan`.
- **Nueva sección `common_config_fields`** para las claves que lee el *motor* del
  `config` de cualquier nodo, sin pertenecer a ningún tipo. Hoy contiene
  `include_extra_info`, que `DagRunUseCase` consulta al armar la salida final.
- **`api_explorer` documentado con `config_fields` vacío** y una nota: se
  construye una sola vez con valores por defecto y su `execute()` recibe
  `_config` sin usar, así que cualquier clave puesta ahí es inerte. Su `schema()`
  anuncia diez campos que el nodo nunca lee — drift del `.rs`, no del catálogo.
- **Correcciones de datos**: `tts.format` acepta también `mpeg` y `ogg` y es
  case-insensitive; `quality` de los nodos de imagen dejó de declarar
  `valid_values` porque el nodo no valida nada y reenvía el string al proveedor
  (con `dall-e-3` el vocabulario es `standard`/`hd`, no `low`/`medium`/`high`);
  y `provider` de `image_generation`/`tts` NO es case-insensitive — el match es
  exacto y `"OpenAI"` falla en runtime.

**Solo documentación.** No cambia ningún comportamiento del motor.

**Documentación de referencia.** [`docs/node_configurations.json`](../docs/node_configurations.json).

**Estado.** done.

---

## 4. El catálogo de nodos deja de ser solo documentación

**Qué cambió.** `docs/node_configurations.json` ahora se embebe en el binario con
`include_str!` y se parsea a tipos (`NodeCatalog`, `NodeCatalogEntry`,
`FieldSpec`) en `dag_engine::domain::lint::catalog`. Se embebe **ese mismo
archivo**, no una copia: el documento que leen las personas y los agentes y el
que consumirá el linter son los mismos bytes, así que no pueden discrepar. El
precedente ya existía en `log_policy.rs`, que embebe una guía para verificar sus
targets de logging.

Tres tests nuevos impiden que el catálogo vuelva a desviarse:

- `declared_node_types_all_have_an_entry` — el archivo no puede contradecirse a
  sí mismo (declaraba 37 tipos válidos y documentaba 32).
- `every_registered_node_type_is_documented_in_the_catalog` — todo tipo que el
  registry sabe ejecutar tiene entrada.
- `the_catalog_documents_no_node_type_the_engine_cannot_run` — y a la inversa.

Los dos últimos construyen el registry **con todas sus dependencias opcionales**:
`secure_suspend` solo se registra si hay `SecureValueService`, y
`image_generation` / `image_edit` / `tts` solo si hay adapter de storage. Un
registry armado sin ellas encogería el conjunto bajo prueba en silencio.

Dos detalles que el tipado obligó a modelar de forma explícita:

- **Obligatoriedad condicional.** `required` no siempre es booleano: `router.schema`
  dice `"mode B only"`. `Requiredness::Conditional` conserva ese valor tal cual, y
  `is_unconditional()` devuelve `false`, para que quien consuma el catálogo no
  pueda tratar una condición no evaluable como un requisito duro.
- **Config abierta.** `input` y `mock_input` emiten su propio `config` como datos
  para los nodos siguientes, así que ninguna clave puede ser "inventada" en ellos.
  El catálogo ya expresaba eso con una clave placeholder entre ángulos
  (`<any_key>`) en `mock_input`; ahora está reconocido en el tipo, vía
  `accepts_any_field()`, y aplicado también a `input`.

**Sin cambio de comportamiento.** Nada consume todavía el catálogo en tiempo de
ejecución: `NodeCatalog::embedded()` no se alcanza desde `run` ni desde `serve`.

**Nota de empaquetado.** `docs/` queda fuera del package root del crate, así que
`cargo package` no podría resolver el `include_str!`. Hoy no es una restricción
—el crate se consume como dependencia git— pero si eso cambiara, el camino es
generar un artefacto dentro del crate.

**Estado.** done.

---

## 5. Vocabulario de diagnósticos para el linter de grafos

**Qué cambió.** Nuevo módulo `dag_engine::domain::lint::diagnostic` con los tipos
en los que se reporta un hallazgo: `Severity`, `DiagnosticCode`, `Diagnostic` y
`LintReport`.

Dos decisiones que quedan fijadas acá, antes de que exista el análisis que las
usa:

- **Los códigos son estables** (`UNKNOWN_FIELD`, `MISSING_REQUIRED_FIELD`, …) y
  se exponen como `&'static str`. Quien consuma los hallazgos —la salida JSON de
  la CLI, o una UI sobre los bindings— debe ramificar sobre el código, nunca
  sobre el texto del mensaje, que es libre de cambiar.
- **`Info` no bloquea.** `has_blocking_findings()` cuenta errores y warnings pero
  ignora `Info`, porque la única severidad `Info` prevista dice "no pude revisar
  este nodo": es una afirmación sobre la cobertura de la herramienta, no sobre el
  grafo. Fallar por eso castigaría al autor por un hueco nuestro.

`LintReport::sort()` ordena por severidad, nodo, campo y código, para que dos
corridas sobre el mismo grafo se lean igual.

**Sin cambio de comportamiento.** Tipos nuevos, sin consumidores todavía.

**Estado.** done.

---

## 6. El análisis del linter de grafos

**Qué cambió.** `dag_engine::domain::lint::linter` — una función pura de grafo a
lista de hallazgos, sin I/O ni acceso al registry: todo lo que necesita llega en
`LintContext`. Detecta campos de config inventados (con sugerencia *did you
mean*), campos obligatorios ausentes, tipos de nodo inexistentes, edges que
apuntan a nodos que no existen, valores fuera del conjunto documentado y tipos
JSON incorrectos.

**Dos puntos de entrada, y la diferencia importa.** `lint_graph_json` recibe el
documento crudo; `lint_graph` recibe un `Graph` ya deserializado. Preferí el
primero siempre que tengas el JSON original: deserializar a `Graph` **descarta en
silencio** toda clave no declarada, así que un nodo con `"default_input_port"`
—una invención real presente en los grafos de ejemplo de este repo— ya no existe
cuando hay un `Graph`. `lint_graph` queda para quien ya tiene uno en la mano.

**Las reglas que evitan el ruido.** Un linter con falsos positivos se ignora, así
que cada una se midió contra los 301 grafos de ejemplo del repo. Sin ellas la
primera versión producía 178 hallazgos de los que 132 eran ruido; con ellas
quedan 252 grafos limpios y 80 hallazgos, todos auditados contra el código.

- **Sin cobertura no se opina.** Un tipo sin entrada en el catálogo produce un
  `NO_CATALOG_COVERAGE` (info) y ni un solo `UNKNOWN_FIELD`.
- **`required` no significa "tiene que estar en `config`".** El edge nombra el
  puerto al que escribe (`"to": "run_sql.query"`); si nombra el campo, no falta.
  Mirar solo "¿tiene algún edge entrante?" producía 35 de 41 avisos falsos. Una
  obligatoriedad condicional nunca se reporta.
- **Nodos de config abierta.** `input` y `mock_input` emiten su config como
  datos: ninguna clave puede ser inventada en ellos.
- **Un comentario no es un ajuste.** Las claves de anotación se ignoran, salvo que
  el tipo de nodo documente un campo con ese nombre.
- **Claves que lee el motor, no el nodo.** `include_extra_info` la lee
  `DagRunUseCase` de cualquier nodo; sin tratarla aparte, el linter la marcaba
  como inventada y afirmaba —falsamente— que el motor la ignora.

**Sin cambio de comportamiento.** `Graph::validate()` no se tocó y nada llama
todavía a estas funciones: la superficie de usuario llega en el cambio siguiente.

**Estado.** done.

---

## 7. `dag_engine lint`: revisar un grafo sin ejecutarlo

**Qué cambió.** Nuevo subcomando:

```bash
cargo run --bin dag_engine -- lint <graph.json> [--format text|json] [--strict]
```

Contesta la pregunta que realmente tiene quien escribe el JSON: cuáles de estos
campos existen y cuáles me los inventé. Hasta ahora un `"modle"` en vez de
`"model"` cargaba bien, pasaba `Graph::validate()` y corría con el modelo por
defecto — el motor deserializa `config` a un `Value` sin tipar y ninguna struct
del grafo usa `deny_unknown_fields`.

**No bloquea nada.** `Graph::validate()` quedó igual y `run` se comporta
exactamente como antes. Los grafos que hoy corren en producción casi con
seguridad contienen campos desconocidos, y volverlos fail-closed rompería agentes
en marcha sin aviso. `--strict` sale con código ≠ 0 para quien lo quiera en CI;
ese es el camino de adopción, no un cambio de default.

**No construye un engine.** Lintear es estático, y exigir conexión a base de
datos para revisar un archivo JSON dejaría la herramienta fuera del alcance de
las personas para las que existe. Los tipos de nodo se toman del catálogo, cuya
correspondencia con el registry está fijada por tests.

**Encontró defectos reales en este repo**, sin ejecutar nada:
`tests/graphs/edge_resolution/default_ports_chain.json` usa `default_input_port`
(que el motor descarta al cargar) y pone `config.left` en nodos `add`/`multiply`
que reciben `_config` sin usar — al correrlo muere con `Entrada no es un número: a`.

**Documentación de referencia.** [`docs/developer_guide/51_graph_linter.md`](developer_guide/51_graph_linter.md).

**Estado.** done.

---

## 8. Bindings: `validate_graph` ahora valida de verdad

**Qué cambió.** `validate_graph` (PyO3) y `validateGraph` (napi) **solo
deserializaban**: no llamaban a `Graph::validate()`, pese a que su doc afirmaba
replicar la estrictez de `cargo run -- run <file>`. Ahora la llaman.

**Cambio de comportamiento.** Cuatro clases de grafo que antes pasaban ahora se
rechazan: node id con `/`, `node_schema` malformado, `memory_mode` inválido o sin
`connection_url`, y bloque `mcp` mal configurado. Los cuatro **ya fallaban al
ejecutar** — el cambio adelanta el error, no invalida nada que antes corriera.

**ADP no afectado, verificado.** `apps/service/ia/platform/` no llama a esa
función en ningún lado; el worker entra por `execute_stream_cancellable` con un
`Graph` ya deserializado. Nota completa en
[`docs/adp_migration/2026-09-02-validate-graph-now-validates.md`](adp_migration/2026-09-02-validate-graph-now-validates.md).

**Guías.** [`48_python_dag.md`](developer_guide/48_python_dag.md),
[`49_typescript_dag.md`](developer_guide/49_typescript_dag.md) y el `.pyi`
actualizados para decir qué valida y qué no.

**Estado.** done.

---

## 9. `lint_graph` / `lintGraph` en los bindings

**Qué cambió.** El linter de grafos queda expuesto a PyO3 y napi. Donde
`validate_graph` contesta *"¿el engine puede cargar esto?"*, `lint_graph`
contesta la pregunta que realmente tiene quien arma el grafo: **cuáles de estos
campos existen y cuáles me los inventé.**

```python
findings = colmena.lint_graph(graph)   # lista de dicts; [] si no hay hallazgos
```

```ts
const findings = lintGraph(graph);     // LintFinding[]
```

Cada hallazgo trae `severity`, `code`, `node_id`/`nodeId`, `field`, `message` y
`suggestion`. **Los `code` son estables** (`UNKNOWN_FIELD`,
`MISSING_REQUIRED_FIELD`, `EDGE_UNKNOWN_NODE`, …) y son lo que hay que consumir;
el `message` es texto para humanos y puede cambiar.

**Advisory.** Los hallazgos nunca impiden ejecutar un grafo; la función solo
lanza si lo que recibe no es un grafo. Es la pieza que permitiría al canvas de
ADP avisar de un campo inventado **antes** de correr el agente.

**Recibe el objeto crudo a propósito.** Deserializar a `Graph` descarta en
silencio toda clave no declarada, así que un nodo con `default_input_port` ya no
existe cuando hay un `Graph` — y esa es justamente una invención real presente en
los grafos de ejemplo del repo. Un test lo fija: `validate_graph` acepta ese
grafo y `lint_graph` reporta `UNKNOWN_NODE_PROPERTY`.

**Aditivo.** Función nueva en ambos bindings, tipo `LintFinding` en la fachada TS
y firma en el `.pyi`. Nada existente cambia de comportamiento → **ADP no
afectado**.

**Guías.** [`48_python_dag.md`](developer_guide/48_python_dag.md) y
[`49_typescript_dag.md`](developer_guide/49_typescript_dag.md).

**Estado.** done.

---

## 10. Linter: dejar de afirmar lo que el catálogo no puede sostener

**Qué cambió.** El linter reportaba *"is not a node type this engine can run"*
para cualquier tipo sin entrada en el catálogo. Esa frase es **falsa** para un
nodo que sí está registrado y solo le falta la entrada — que es la forma más
probable de que aparezca un tipo desconocido: alguien agrega el nodo a
`registry.rs`, olvida el catálogo y corre `lint` antes que los tests.

`LintContext` ahora lleva un `KnownNodeTypes` que separa los dos grados de
certeza, porque deciden qué le está permitido afirmar:

| Variante | Ante un tipo desconocido |
|---|---|
| `Registry(&set)` | `UNKNOWN_NODE_TYPE` (error), *"is not a node type this engine can run"* — la ausencia es prueba |
| `CatalogOnly` | near-miss → `UNKNOWN_NODE_TYPE` (error), *"is not a documented node type"*; si no → `NO_CATALOG_COVERAGE` (info) |
| `Unchecked` | no opina |

La CLI usa `CatalogOnly`. El typo (`llm_kall` → `llm_call`) sigue siendo un error
con su sugerencia; lo que cambió es que un tipo genuinamente nuevo ya no recibe
una afirmación inventada sobre el motor, y sus campos no se marcan como
inventados. Nuevo constructor `LintContext::from_registry` para quien tenga el
registry a mano.

Efecto lateral bienvenido: `NO_CATALOG_COVERAGE` pasa a ser alcanzable desde la
CLI. Antes era inalcanzable por construcción, y la guía 51 lo documentaba como
limitación.

**Y `compact()` dejaba comillas desbalanceadas.** Truncaba la forma ya
entrecomillada, así que un valor largo salía como `"xxxxx...` y se leía como un
string sin cerrar. Ahora trunca el contenido y después entrecomilla.

**Sin cambio de API pública** más allá del `LintContext` que introdujo el propio
linter en esta misma serie, y que todavía no tiene consumidores fuera del repo →
**ADP no afectado**.

**Documentación.** [`51_graph_linter.md`](developer_guide/51_graph_linter.md),
sección "De dónde sale la autoridad".

**Estado.** done. Cierra la fase 1.

---

## 11. `api_explorer`: el `schema()` dejó de anunciar config que no lee

**Qué cambió.** El `schema()` de `api_explorer` listaba un bloque `config` con diez
campos (`enable_cache`, `cache_ttl_seconds`, `fuzzy_match_threshold`, …). **El nodo
no lee ninguno**: se construye una sola vez al registrarse con
`ApiSpecUseCaseConfig::default()` y su `execute` recibe `_config` sin usar. Un
operador que ponía cualquiera de esos campos no obtenía efecto — y, ahora que el
linter existe, un grafo que confiara en ellos recibiría un `UNKNOWN_FIELD` contra
un `schema()` que los prometía.

Se **eliminó** el bloque `config`, igual que se hizo con el flag fantasma
`guardrail_enabled` de `sql_query` (§1). Ahora el `schema()` coincide con la
entrada del catálogo, que ya documentaba `config_fields` vacío más un
`config_note`. Un test fija que `schema()` no anuncie `config`.

**Sin cambio de comportamiento.** El bloque `config` del `schema()` es puramente
descriptivo — los tres consumidores de `schema()` leen solo `inputs`. → **ADP no
afectado**.

**Estado.** done. Último pendiente de la fase 1 del linter.

---

## 12. Fase 2 del linter: el código empieza a ser dueño de los campos

**Qué cambió.** Nuevo método `ExecutableNode::config_schema() -> Option<NodeCatalogEntry>`
(default `None`) con el que un nodo declara, en código, qué campos de config
acepta. Un test cruza esa declaración contra `docs/node_configurations.json` y
falla si divergen, así que para un nodo migrado el catálogo deja de ser
"documentación mantenida a mano" y pasa a ser **demostrablemente correcto**.

**Solo hechos mecánicos.** `config_schema()` declara nombres de campo,
`required`, `valid_values` y `read_only` — lo único que el linter verifica. La
prosa (`description`, `example`, `default`) sigue viviendo en el JSON a
propósito: es lo que leen humanos y agentes, y meterla en literales de Rust
volvería cada mejora de doc un recompilado. No se generará el JSON completo.

**Aditivo y por lotes.** El default `None` significa "todavía no declarado —
el catálogo sigue siendo su autoridad", así que la migración es nodo por nodo y
no rompe ninguna implementación. Este cambio migra **9 de 37**: los ocho nodos
sin config (`log`, `output`, `current_time`, `api_explorer`, `add`, `subtract`,
`multiply`, `divide`) y `exponential` (un campo `exponent` requerido), que prueba
las dos formas — entrada vacía y entrada con un campo tipado.

El test es no-vacuo por construcción: exige un mínimo de 9 nodos comprobados, y
se verificó por mutación que falla si un campo del código deja de ser `required`
o si el código inventa un campo que el catálogo no tiene.

**Sin cambio de comportamiento.** El linter sigue leyendo el catálogo; nada
consume `config_schema()` en runtime todavía. `NodeCatalogEntry`/`FieldSpec`
ganaron `PartialEq` y constructores fluidos, ambos aditivos → **ADP no afectado**.

**Documentación.** [`51_graph_linter.md`](developer_guide/51_graph_linter.md),
"Limitaciones conocidas".

**Estado.** partial — 9/37 nodos; el resto (incluidos config abierta y
`reserved_input_keys`) en próximos slices. Ver BACKLOG.

