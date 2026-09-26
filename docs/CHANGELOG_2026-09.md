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

---

## 13. Fase 2, slice 2: las tres formas que el builder no sabía expresar

**Qué cambió.** El builder de `config_schema()` solo podía declarar campo/tipo/
`required`/`valid_values`/`read_only`. El catálogo usa tres cosas más, y sin ellas
cuatro nodos no se podían migrar:

| Primitiva | Para qué | Nodo que la ejercita |
|---|---|---|
| `NodeCatalogEntry::open_config()` | nodos cuya config entera es dato — placeholder `<any_key>`, ahora la constante `ANY_FIELD_KEY` | `mock_input`, `input` |
| `NodeCatalogEntry::with_reserved_input_keys()` | claves que el motor se reserva en ese nodo | `http_request` (13) |
| `FieldSpec::conditional(v)` | obligatoriedad que el catálogo enuncia en prosa | `router.schema` (`"mode B only"`) |

**Migrados 13 de 37** con estos cuatro: `mock_input`, `input`, `router` y
`http_request`.

**`http_request` deriva sus reserved keys de la constante real** que el nodo ya
usa para filtrar parámetros salientes (`Self::RESERVED_KEYS`), en vez de repetir
la lista. Si alguien agrega una clave ahí y olvida el catálogo, el test de drift
falla — que es exactamente lo que la fase 2 busca.

**Auditoría, que es el punto de migrar.** Se verificó campo por campo contra el
código antes de declarar. Esta vez el catálogo estaba bien: los 14 campos de
`http_request` son reales (cuatro se leen vía `limit_usize`/`limit_bool`, `auth`
es el bloque OAuth config-only, y `secure` lo consume `SecureValueService`), y
`router` lee 7 más `temperature`, que está hardcodeada en 0.1 y por eso figura
`read_only`.

**Sin cambio de comportamiento.** El linter sigue leyendo el catálogo;
`config_schema()` solo se cruza en el test. Todo aditivo → **ADP no afectado**.

**Estado.** partial — 13/37. Faltan 24, ya sin primitivas nuevas por delante.
Ver BACKLOG para la clasificación por dificultad.

---

## 14. Fase 2, slice 3: los 9 nodos fáciles — y dos campos fantasma que destapó

**Qué cambió.** Migrados a `config_schema()`: `secure_suspend`, `subgraph`,
`python_script`, `trigger_webhook`, `suspend`, `loop_controller`,
`document_read`, `task_memory_writer` y `document_edit`. **Van 22 de 37.**

`subgraph` deriva sus dos fuentes de grafo hijo de `CHILD_GRAPH_SOURCE_KEYS`, la
constante que el propio nodo usa para buscarlas — mismo patrón que
`http_request` con sus reserved keys.

### Dos campos que el catálogo documentaba y nadie lee

Auditar antes de declarar es el punto de la fase 2, y esta vez apareció esto:

**`secure_suspend.id` — eliminado.** `effective_config` mezcla un `id` que viene
de `inputs`, pero lo único que lo consume es `parse_and_validate_secrets`, que
lee `secrets` y de cada entrada su `name`/`question`. El `id` nunca se lee. Es
coherente con lo que ya decía CLAUDE.md: en `secure_suspend` el id de cada
pregunta es `secrets[].name`, no un `config.id`. Ningún grafo del repo lo usaba.

**`trigger_webhook.method` — acotado a `["POST"]`, no eliminado.** El motor
**no lee** esta clave: `api::serve_dag` registra toda ruta de `trigger_webhook`
como POST incondicionalmente. Pero el catálogo la declaraba aceptando
`GET/POST/PUT/DELETE/PATCH`, o sea invitaba a escribir un método que después se
ignora en silencio — una trampa latente.

Eliminarla habría producido **123 hallazgos** de golpe (los 123 grafos del repo
que la setean), todos sobre config que nunca hizo daño: los 123 escriben
`"POST"`, que es justo lo que el motor hace. Acotar `valid_values` a `["POST"]`
dice la verdad, deja el corpus en cero hallazgos nuevos, y ahora sí marca el caso
que importa: un `"GET"` sale como `INVALID_FIELD_VALUE`. La descripción explica
que la clave no se lee.

**Sin cambio de comportamiento del motor** y sin ruido nuevo: el linter da los
mismos 80 hallazgos sobre los 300 grafos de ejemplo. El piso del test de drift
subió de 13 a 22.

**Estado.** partial — 22/37. Faltan 15: 9 medianos y 6 caros.

---

## 15. Fase 2, slice 4: los 9 medianos — y 7 campos de documents sin documentar

**Qué cambió.** Migrados a `config_schema()`: `sql_query`, `output_parser`,
`for_each`, `document_create`, `tts`, `image_generation`, `image_edit`,
`socketio_request` e `information_extraction`. **Van 31 de 37.**

### El catálogo documentaba 2 de los 9 campos de almacenamiento de documents

`DocumentRuntime::from_config` —al que los tres nodos de documents le pasan la
config del nodo (`document_nodes.rs:54`)— lee **nueve** campos:
`storage_backend`, `storage_root`, `asset_storage_root`, `gcs_bucket`,
`gcs_prefix`, `asset_gcs_prefix`, `default_retention`, `max_asset_size_bytes` y
`allowed_asset_mimes`. El catálogo documentaba **los dos primeros**.

Consecuencia concreta: cualquier grafo que configurara documents contra GCS
—`gcs_bucket`, `gcs_prefix`— recibía `UNKNOWN_FIELD` del linter sobre una
configuración real y funcionando. Falsos positivos sobre un camino soportado, que
el propio catálogo declara válido en `storage_backend.valid_values` (`localfs`,
`gcs`).

Los siete faltantes se agregaron a `document_create`, `document_edit` y
`document_read`. Detalle revelador: la descripción de `storage_root` ya
mencionaba `asset_storage_root` — quien escribió el doc conocía el campo y nunca
lo documentó como tal.

### Una falsa alarma que conviene registrar

`sql.rs` lee `provider`, `model` y `api_key`, pero **del objeto anidado
`guardrail_llm`**, no de la config del nodo. El catálogo tiene razón con sus 6
campos. Un barrido de `*.get("...")` filtrando por receptores que contienen
`cfg`/`config` los atribuye al nodo por error; hay que mirar el contexto.

**Sin ruido nuevo**: los mismos 80 hallazgos sobre los 300 grafos de ejemplo. El
piso del test de drift subió de 22 a 31.

**Estado.** partial — 31/37. Faltan 6, todos del grupo caro.

---

## 16. Fase 2, slice 5: el clúster LLM y `tavily_client`

**Qué cambió.** Migrados a `config_schema()`: `planner`, `critic`, `reactor`,
`orchestrator` y `tavily_client`. **Van 36 de 37** — solo falta `llm_call`.

**`planner.texts` no estaba documentado.** El planner lee `config.texts`
(`planner.rs:275`) igual que `critic` y `reactor`, pero el catálogo solo lo
documentaba en esos dos. Agregado. Ningún grafo del repo lo usaba, así que el
falso positivo era latente, no activo.

**Las tres `temperature` son genuinamente `read_only`.** `planner` y `critic`
llaman al LLM con 0.1 y `reactor` con 0.2, todas hardcodeadas. El `read_only` del
catálogo describe la realidad.

**`orchestrator` declara sus cuatro sub-bloques con las constantes `KEY_*`** que
el propio nodo usa para buscarlos, igual que `http_request` con sus reserved keys
y `subgraph` con sus fuentes de grafo hijo. Renombrar una constante sin tocar el
catálogo hace fallar el test.

**`tavily_client`**: sus 18 campos se separan en dos grupos que la declaración
documenta — nueve ajustes propios del nodo, y nueve argumentos de sub-tool que
`build_effective_inputs` rellena desde config cuando el nodo corre como nodo de
grafo en vez de como tool del LLM.

**Falsa alarma verificada**: `orchestrator` lee `final_reactor` vía
`KEY_FINAL_REACTOR` en una línea aparte del `.get(`, y un barrido ingenuo la
pierde. El campo es real y requerido.

**Sin ruido nuevo**: los mismos 80 hallazgos sobre los 300 grafos. Piso del test
de drift: 31 → 36.

**Estado.** partial — 36/37.

---

## 17. Fase 2 COMPLETA: `llm_call` declarado, y `session_id` era una promesa vacía

**Qué cambió.** `llm_call` declara sus 33 campos. **Los 37 tipos de nodo tienen
`config_schema()`**, así que el catálogo ya no puede desviarse del código en el
set de campos ni en sus hechos mecánicos.

### `llm_call.session_id` no hacía nada desde abril

El catálogo prometía:

> *"When provided together with 'connection_url', enables persistent conversation
> memory — the message history is stored in the database and loaded on subsequent
> calls with the same session_id."*

**Falso.** `conversation_key` (`llm.rs:1406`) se arma con
`inputs.__colmena_agent_session_id`, `inputs.__colmena_session_id` y
`inputs.__colmena_node_id_path`. No existe ningún `config.get("session_id")` en
`llm.rs`, y el `__colmena_session_id` lo inyecta el motor desde el id efímero del
run (`run_use_case.rs:531`), nunca desde la config del nodo.

**Fue una regresión de documentación, no un bug de código.** El campo SÍ estuvo
cableado, y lo desconectó a propósito `fc46c4db` (2026-04-28), *"switch llm_call
to (agent_session_id, node_id_path) keying"*: con agente presente el historial se
filtra por `(agent_session_id, node_id)` —mismo chat entre runs—, y sin agente cae
a `(session_id, node_id)`, aislando cada run. El catálogo nunca se actualizó.
Cablearlo de vuelta desharía esa decisión, así que se corrigió el documento.

La guía canónica ya decía lo correcto
([`15_memory_guide.md:141`](developer_guide/15_memory_guide.md)): el id de la
memoria *"lo deriva el engine automáticamente del run actual — no lo configurás
vos en el nodo"*, y para persistir entre runs va `agent_session_id`. Esa misma
guía ya registra un fantasma idéntico y anterior (`thread_id`).

**Efecto medible**: el linter pasa de 80 a 110 hallazgos sobre los 300 grafos de
ejemplo. Los 30 nuevos son todos `session_id`, y son verdaderos: esos grafos
llevan config inerte que hace creer a su autor que tiene memoria persistente.
Limpiarlos queda anotado en BACKLOG.

### `skills_paths` faltaba en el catálogo

`llm_call` lee `skills_path` (un directorio) **y** `skills_paths` (varios), y
deduplica por nombre (`llm.rs:712-722`). El catálogo solo documentaba el singular.
Agregado.

### Sobre la verificación de este slice

La declaración de 33 campos se transcribió desde el catálogo, lo que vuelve el
test de drift tautológico **para este nodo**. Por eso se verificó aparte que el
conjunto declarado coincide exactamente con el que `llm.rs` lee de `config`:
33 = 33, sin sobrantes ni faltantes.

**Estado.** done — 37/37.

---

## 18. El motor valida el grafo en toda entrada, no sólo desde el CLI

**Qué cambió.** `Graph::validate()` ahora corre en `DagRunUseCase::execute_stream`,
el único punto donde convergen todas las entradas. Antes sólo validaba el CLI
(`api.rs`): las cuatro entradas de librería —`execute_stream`,
`execute_stream_cancellable`, `run_dag`, `stream_sse_parts`— recibían un `Graph`
y lo ejecutaban sin verificar, y **ésa es la que usa el worker de ADP**.

Cerrado el hueco que quedó abierto al arreglar los bindings en §8: los bindings
validaban, pero ADP no pasa por ellos.

**Por qué el riesgo es menor de lo que parece.** De las cuatro cosas que
`validate()` rechaza, dos ya fallaban igual más adelante: `node_schema` se
re-parsea al construir tools (`dag_tool_executor.rs:928`) y `memory_mode` se
re-verifica ahí mismo (`:922`). Para ésas esto sólo adelanta el error y mejora el
mensaje. Las otras dos —bloque `mcp` mal configurado y node id con `/`— fallaban
**en silencio**: un servidor MCP mal configurado simplemente se ignoraba, que
para el operador se lee como "el modelo ignoró mi servidor".

**Válvula de seguridad**: `COLMENA_GRAPH_VALIDATION=off`, misma forma que
`COLMENA_PREFLIGHT_HEALTH=off`.

**Sobre el test.** El que existía llamaba a `g.validate()` a mano — habría pasado
feliz mientras nada la llamaba, que era exactamente el estado a corregir. El
nuevo maneja `DagRunUseCase::execute_stream`, y corre en CI (no `#[ignore]`)
porque un registry vacío alcanza: la validación ocurre antes de buscar ningún
nodo. Se verificó por mutación que falla si se quita la llamada.

Un detalle que costó encontrar: los dos tests que dependen de
`COLMENA_GRAPH_VALIDATION` se pisaban entre sí, porque `set_var` es global al
proceso y CI corre en paralelo. El síntoma era un error de nodo no encontrado que
parecía cableado roto. Resuelto con un lock explícito, no con `--test-threads=1`,
que sólo lo habría escondido.

**Nota de migración para ADP**:
[`2026-09-03-graph-validated-on-every-entry.md`](adp_migration/2026-09-03-graph-validated-on-every-entry.md).

**Estado.** done.

---

## 19. `short_ulid` truncaba el ULID a 12 chars y dejaba 2 de azar

**Qué cambió.** `UlidIdGenerator::short_ulid()`
(`documents/infrastructure/ids.rs`) construía el cuerpo de cada id así:

```rust
let ulid = ulid::Ulid::new().to_string();
ulid[..12].to_ascii_lowercase()
```

Un ULID son 26 chars Crockford base32: **10 de timestamp (48 bits de ms) + 16 de
aleatoriedad (80 bits)**. Cortar en `[..12]` conserva el timestamp entero y deja
**2 chars de azar = 1024 valores distintos por milisegundo**. Todos los ids del
módulo pasan por ahí — `art_`, `sheet_`, `tbl_`, `blk_`, `run_`, `row_`, `li_`,
`sl_`, `asset_` — y son **ids persistidos**, así que esto era un defecto de
integridad de datos, no un problema de tests.

Tasa de colisión medida sobre ids emitidos seguidos dentro del mismo milisegundo:

| ids | probabilidad de colisión |
|---|---|
| 8 | 2.9% |
| 16 | 11.4% |
| 32 | 38.0% |
| 64 | 86.0% |

**Cómo se manifestó.** El job "Test (3.12)" del PR #262 —un cambio de solo
documentación— falló en `html_documents_e2e.rs` con
`IRValidationFailed { path: "/slides/sl_01m1md27rf28/blocks/blk_01m1md27rgw0",
reason: "duplicate block id (across all slides)" }`. Los dos ids comparten el
prefijo de timestamp `01m1md27r`: mismo milisegundo. Pasaba 60/60 en macOS y
fallaba de forma intermitente en CI, y por eso se leía como flaky. No era flaky:
era una carrera real que las máquinas más rápidas pierden más seguido.

**El arreglo.** El cuerpo pasa a tener **22 chars**: 10 de timestamp, 8 de
aleatoriedad (40 bits) y 4 de una **secuencia local al proceso** (`AtomicU64`,
codificada en el mismo alfabeto Crockford en minúscula).

Los tres tramos cubren cosas distintas:

- El **timestamp** mantiene los ids aproximadamente ordenables, como antes.
- La **secuencia** vuelve la unicidad *estructural* dentro de un proceso, no
  apenas probable: dos ids solo pueden repetirse si la secuencia da la vuelta, lo
  que exige 2^20 ids dentro de un mismo milisegundo. Este es exactamente el caso
  que rompía el test E2E, donde un documento entero se arma en una sola ráfaga.
- Los **40 bits de azar** cubren el caso entre procesos, donde no hay contador
  compartido.

**Sin consumidores afectados.** Se verificó antes de cambiar el largo que nada
asume 12 chars: no hay regex de id, ni validación de largo, ni ids generados
hardcodeados en fixtures o snapshots. Las únicas comprobaciones sobre ids miran
el prefijo semántico (`starts_with("blk_")`). Los renderers de HTML, Word y Excel
tratan el id como opaco. Los ids ya persistidos siguen siendo válidos: el formato
nunca se validó, así que ids viejos de 12 chars y nuevos de 22 conviven sin
migración.

**La cobertura anterior era vacua.** Los cuatro tests que ya existían comparaban
**dos** ids (`assert_ne!(g.new_artifact_id(), g.new_artifact_id())`) — con 1024
valores por ms eso falla ~0.1% de las veces, o sea casi nunca. Los dos tests
nuevos emiten 20 000 ids en un loop apretado y 16 000 desde 8 hilos concurrentes.
Se verificó que **fallan** contra la implementación vieja (5855 de 20 000 ids
colisionaron) y pasan contra la nueva; no son tautológicos.

**Documentación de referencia.** `docs/superpowers/specs/2026-04-21-documents-feature-design.md` §5.5,
`docs/agent_context/audit/src__libs__colmena__src__documents__infrastructure__ids.rs.md`.

**Estado.** done.

---

## 20. Se borró el `session_id` inerte de `llm_call`, y cargaron los tres grafos rotos

**Qué.** Dos pendientes que el linter había dejado anotados en `BACKLOG.md` y que se
cierran juntos porque los dos son la misma clase de problema: un JSON que dice algo que
el motor no hace.

### El `session_id` que prometía memoria

§17 quitó `session_id` de `llm_call` en `docs/node_configurations.json` tras verificar que
el nodo lee `__colmena_session_id` / `__colmena_agent_session_id` inyectados por el motor
y **nunca** el del `config`. Quedaba el rastro. Este cambio lo borra de las cuatro partes
donde seguía vivo:

1. **30 apariciones en 27 grafos de ejemplo** bajo `tests/graphs/`. Todas eran `llm_call`;
   se comprobó nodo por nodo antes de tocar nada, porque `document_create` **sí** lee
   `config.session_id` (`document_nodes.rs:42`) y no debía barrerse con la misma escoba.
2. **`LlmNode::schema()`**, que lo anunciaba dos veces: en el bloque `config` y en el
   bloque `inputs`. Lo segundo era lo grave — `dag_tool_executor.rs` convierte
   `schema()["inputs"]` en los parámetros de la tool, así que un `llm_call` usado como
   herramienta le ofrecía al modelo un parámetro `session_id` descrito como *"enables
   memory"* que no hacía nada. Era opcional, nunca estuvo en `required`, y ningún test
   lo afirmaba.
3. **`llm_call.input_ports` del catálogo**, que lo declaraba como *"Dynamic session ID for
   memory"*. El linter no lo había visto porque sólo cruza `config_fields`.
4. **Dos guías** que lo enseñaban: el "Ejemplo 2: Con Memoria Conversacional" de
   `14_llm_deep_dive.md` y la respuesta sobre persistencia de `16_data_flow_guide.md`.
   Ambas pasan ahora a `--agent-session-id`, que es la forma que sí funciona.

`connection_url` **se queda**: ese sí lo lee el nodo (`llm.rs:1454`), y sin él la memoria
es sólo en proceso.

### Los tres grafos que no deserializaban

`forward_generated_artifact.json`, `upload_inline_to_endpoint.json` y
`upload_signed_url_to_endpoint.json` declaraban `nodes` como array; `Graph` lo espera como
`HashMap<String, NodeConfig>`, así que fallaban con `invalid type: sequence, expected a
map` antes de llegar a ejecutarse.

Pasarlos a mapa y **correrlos** destapó siete defectos más. Ninguno era visible mientras el
archivo no cargaba, y ninguno lo habría encontrado sólo mirar el JSON:

1. **`system_prompt`** en los tres. `llm_call` lee `system_message` (`llm.rs:1373`);
   `system_prompt` no lo lee nadie. El prompt de sistema entero se descartaba en silencio.
2. **`"type": "trigger"`** en los tres. El motor no registra `trigger` — registra
   `trigger_webhook` (`registry.rs:102`). El grafo hermano que sí funciona
   (`agent_multipart_upload.json`) usa `trigger_webhook` con el mismo id `trigger`, que es
   de donde salió la confusión. Se les puso además un `test_payload` para que el `run`
   local tenga con qué arrancar.
3. **Sin `api_key`** en el `llm_call`. Lo reportó el linter en cuanto el archivo cargó.
4. **`fixed_config` muerto.** Los tres declaraban `url`/`method`/`headers` en
   `fixed_config` **y** un `node_schema` para `body`. `dag_tool_executor.rs:1976` toma
   `node_schema` como PATH 0 y la rama de `fixed_config` es un `else if`: con `node_schema`
   presente, el `fixed_config` entero no se lee. Es exactamente el anti-patrón que
   `CLAUDE.md` marca como *WRONG — mixing*; ahora la plomería va como campos `fixed`
   dentro del `node_schema`.
5. **`url` no es un campo de `http_request`.** El nodo arma la URL con `base_url` +
   `endpoint` (`http.rs:860-892`), y ambos caen a `""` si faltan. El síntoma era
   `Invalid URL '': relative URL without a base`. El catálogo ya lo tenía bien; el grafo
   no.
6. **`attachment_id`** en `forward_generated_artifact.json`, en el prompt y en la
   descripción de la tool. Plan B lo retiró el 2026-05-25: `image_generation` devuelve
   `document_id` y nada más (`image_generation.rs:391`). El agente venía instruido a leer
   una clave inexistente.
7. **Cobertura imaginaria.** La descripción de los tres afirmaba ser usada por
   `tests/attachment_uniform_resolution_test.rs`, y una agregaba que *"el test reescribe la
   url"*. Ese test no carga ningún `.json`: maneja `HttpNode::execute` directamente. Su
   propio encabezado los llama "companion graphs"; la descripción del grafo convirtió eso
   en una afirmación de cobertura que nunca fue cierta.

Los defectos 4 y 5 son los que importan: entre los dos, el `http_request` salía con la URL
vacía. Un `dag_engine lint` limpio **no** los habría encontrado — el linter revisa el
`config` del nodo, no el `node_schema` de una tool.

**Verificación.** Los tres cargan y pasan `dag_engine lint` sin hallazgos.
`forward_generated_artifact.json` se ejercitó de punta a punta contra un endpoint real
(una copia apuntando a `httpbin.org/post`): el agente generó la imagen, el
`$attachment:<document_id>` se resolvió, y el POST multipart devolvió `200` con la parte
`file` de tipo `image/png` en la respuesta. Contra `kb.test` —el placeholder que el archivo
commiteado conserva— llega hasta el DNS, que es lo correcto. Los otros dos terminan con el
agente contestando que no hay documento adjunto: el CLI no tiene forma de registrar uno,
eso lo hace la aplicación anfitriona, así que su ejecución completa **no** se verificó y no
se afirma.

**Sobre ADP.** Quitar `session_id` del bloque `inputs` cambia el JSON schema de la tool que
se le manda al proveedor cuando un `llm_call` se expone como herramienta: desaparece un
parámetro opcional que no tenía efecto. Ningún grafo del repo lo listaba en
`exposed_inputs`. Un grafo persistido que traiga `session_id` en el `config` sigue
cargando igual — el linter lo reporta como campo desconocido, que es exactamente lo que
es.

**Estado.** done.

---

## 21. El linter entra en `tool_configurations`: primera regla, la de precedencia

**Qué.** Primera de tres rebanadas que cierran el hueco que abrió la §20: el
linter revisaba el `config` de cada nodo pero no miraba el `node_schema` ni el
`fixed_config` de sus tools, así que daba `no findings` sobre un grafo que no
funcionaba.

Esta rebanada agrega el recorrido y una sola regla, `DEAD_FIXED_CONFIG`.

### Por qué el recorrido lee el JSON crudo

`ToolConfiguration` no lleva `deny_unknown_fields`, así que una clave inventada
dentro de una entrada de tool se descarta al deserializar y ya no existe cuando
hay un `Graph`. Es exactamente la razón por la que existe `lint_graph_json`
(§7), y por eso el nuevo walker cuelga de ahí y no de `lint_graph`. Un test fija
esa asimetría para que los dos puntos de entrada no vuelvan a prometer lo mismo
sin darlo, como pasó con `validate_graph` (§9).

### La regla

`DagToolExecutor` arma los argumentos en un `if`/`else if`: `node_schema` es
PATH 0 y `fixed_config` sólo se alcanza si el primero está ausente
(`dag_tool_executor.rs:1976`). Con los dos presentes, el `fixed_config` **entero**
se descarta — no sólo las claves que colisionan.

El mensaje nombra cada clave perdida, porque la pregunta siguiente del autor
siempre es *cuál* de mis ajustes desapareció:

```
error [DEAD_FIXED_CONFIG] node "agent".tool_configurations.http_upload.fixed_config:
  … discards "fixed_config" entirely, so "url", "method", "headers" and
  "allow_http_urls" never reach the node
```

Un `fixed_config` vacío no se reporta: descartar nada no cuesta nada.

### Ruido medido antes de escribir la regla

Sobre las **206 entradas** de `tool_configurations` del corpus, la regla dispara
**cero** veces — los únicos tres casos que existían se corrigieron en la §20. El
corpus queda en 80 hallazgos, idéntico al baseline.

Que no dispare hoy no la hace inútil: se reconstruyó el estado intermedio del
grafo roto de entonces (ya como mapa, todavía con el `fixed_config` muerto) y se
linteó con las dos versiones del binario. `develop` dice `no findings`; con esta
rebanada sale el error nombrando las cuatro claves. Ese es el hueco, demostrado
en vez de argumentado.

### Una mutación encontró peso muerto

De cinco mutaciones aplicadas a la regla, cuatro matan tests. La quinta —invertir
el orden en que el walker emite las entradas— **no mató nada**, y tenía razón:
`LintReport::sort` ya ordena por `(severity, node_id, field)` y el `field` de
cada hallazgo lleva el nombre de la tool, así que ordenar dentro del walker era
peso muerto. Se quitó, y el test que parecía cubrirlo se renombró para decir qué
garantiza de verdad.

### Y la revisión encontró el caso que las mutaciones no podían

La primera versión de la regla exigía que el `node_schema` fuera un objeto **no
vacío**. El executor no: su rama es `if let Some(schema)`, y como `NodeSchema` es
un `HashMap`, `"node_schema": {}` deserializa a `Some(mapa vacío)` y toma PATH 0
igual. Un grafo con `"node_schema": {}` junto a un `fixed_config` poblado perdía
todo el `fixed_config` en runtime y el linter decía `no findings` — un falso
negativo en la clase exacta de defecto para la que se escribió la regla, y encima
la guía afirmaba lo contrario sin condición.

Lo encontraron **dos lenses por separado** (`review-readability` y
`review-reliability`), y se reprodujo con el binario antes de aceptarlo. Las
cinco mutaciones previas no podían: todas atacan código escrito, y el problema
era un caso **no** escrito. Un test ausente no tiene mutación que lo mate.

La regla ahora mira presencia (`is_some_and(Value::is_object)`), no contenido.
`null` sigue sin reportarse: es el único valor que deserializa a `None` y deja el
`fixed_config` vivo de verdad. Dos tests nuevos fijan las dos ramas, y una sexta
mutación —restaurar el `!is_empty()` original— mata el primero.

Una segunda pasada de revisión sobre el candidato ya corregido encontró que el
arreglo traía su propio exceso de afirmación: la guía decía que `null` era la
**única** forma que dejaba el `fixed_config` vivo. Falso en un tercer caso — un
`node_schema` que sea string, número o array hace que `tool_configurations` no
parsee y el nodo falle entero (`llm.rs`), así que el `fixed_config` no está ni
vivo ni descartado: no corre nada. La misma frase estaba repetida en un comentario
del código y en el docstring de un test. Los tres se corrigieron, y la guía ahora
lleva una tabla con los **tres** comportamientos en vez de dos. Que una entrada de
tool malformada no tenga diagnóstico propio quedó anotado en `BACKLOG.md`.

Una **tercera** pasada encontró que la corrección anterior seguía incompleta, y por
la misma causa: describí el comportamiento del executor ignorando que
`Graph::validate()` corre antes, en toda entrada del motor — cableado que se hizo
en el §18, en esta misma serie. `validate()` deserializa el `node_schema` crudo a
`NodeSchema`, que es un `HashMap` **pelado** y no un `Option`, así que rechaza el
grafo al cargar cuando el valor es `null`, un escalar, un array, o un objeto con
un campo anidado inválido. Dos consecuencias: `"node_schema": null` **no** deja el
`fixed_config` vivo —sólo la ausencia lo hace—, y la afirmación del BACKLOG de que
una entrada malformada "no tiene diagnóstico" era falsa: lo tiene, sólo que no es
un hallazgo del linter. La guía pasa a una tabla de dos compuertas
(`validate()` primero, executor después), y el test cuyo nombre prometía un
`fixed_config` vivo se renombró a lo que fija de verdad: el silencio de la regla.

Vale registrar el patrón, porque se repitió: **afirmé un límite sin enumerar los
casos**. La primera corrección sí tocó la lógica —el guard pasó de exigir un
schema no vacío a mirar presencia—; las siguientes fueron de documentación y de
nombres. Una cuarta pasada encontró la misma falla una vez más, ahora en la
cobertura: la fila de los escalares estaba documentada y sin test. Quedó fijada,
junto con la del objeto de campo anidado inválido, así que hoy cada fila de la
tabla tiene una prueba que la sostiene.

La otra observación fue contra un test: `..._are_skipped_rather_than_panicking`
prometía probar ausencia de pánico, pero `Value::get` y `Value::as_object` son
totales —devuelven `None` sobre un string, un array o `null`— así que ninguna
disposición de esos fixtures distingue el código con guards del código sin ellos.
Se renombró a lo que sí fija: que una entrada malformada no produce hallazgo.

**Alcance.** Aditivo. Un `DiagnosticCode` nuevo y una llamada más en
`lint_graph_json`. Ningún grafo existente cambia de resultado. Las dos rebanadas
que faltan —campos cruzados contra el tipo de nodo destino, y tools sintéticas en
`KnownNodeTypes`— siguen en `BACKLOG.md`.

**Estado.** done.

---

## 22. El linter revisa los campos de una tool contra el nodo al que apunta

**Qué.** Segunda de las tres rebanadas abiertas por la §20. La §21 revisó la
*forma* de una entrada de `tool_configurations`; ésta revisa su *contenido*: cada
clave de `node_schema`, `fixed_config` y `node_config` se cruza contra el
`node_type` destino. Cubre el defecto que quedaba: `url` declarado en
`http_request`, cuyos campos son `base_url` y `endpoint`.

### El set válido no es `config_fields`

Un nodo despachado como tool recibe sus claves configuradas como **inputs**. El
set correcto es `config_fields` + `input_ports` + `reserved_input_keys`. Medido
sobre el corpus antes de escribir la regla: contra `config_fields` solo, 16 grafos
que funcionan se reportan como rotos (`task` en `subgraph`, `rows` y `user` en
`python_script`).

### Tres severidades, decididas por el catálogo

La pregunta no es "¿está declarada?" sino "¿qué hace el nodo con una clave que no
declara?", y eso ya lo dice el catálogo con sus claves placeholder:

| El tipo de nodo | Placeholder | Diagnóstico | Nodos |
|---|---|---|---|
| Acepta cualquier clave | `<any_key>`, `<any_text>` | nada | 5 |
| La reinterpreta | `<extra_keys>` | `REPURPOSED_TOOL_FIELD` (warning) | 1 |
| La ignora | ninguno | `UNKNOWN_FIELD` (error) | 31 |

La fila del medio es la del bug real. `http_request` convierte toda clave no
reservada en query param (`http.rs:264`), así que `url` no se ignoraba: salía como
`?url=…` contra una URL base vacía. Llamarlo "campo inventado" habría sido mentira,
así que el mensaje dice qué pasa de verdad.

Que la regla lea la severidad del catálogo y no de una lista en el código tiene una
consecuencia útil: un tipo de nodo nuevo obtiene el veredicto correcto
documentándose, sin tocar el linter. Un placeholder que el linter no conozca cae a
"acepta cualquier cosa" — el silencio es el veredicto seguro cuando el catálogo
describe algo que esta regla no aprendió.

### Dos decisiones de diseño

**`input_ports` vive fuera de `NodeCatalogEntry`.** El cruce de la fase 2 compara
la entrada entera contra el `config_schema()` de cada nodo, y el alcance acordado de
esa declaración es el *config*. Meterlos adentro obligaría a los 37 nodos a declarar
un segundo eje sin ganancia a nivel de nodo, así que el catálogo los guarda en un
mapa lateral y deserializa el documento dos veces detrás del `OnceLock`.

**Se revisa `node_config`.** Es el único bloque que usan las entradas toolkit: las
15 del corpus configuran su nodo por ahí y nunca por `fixed_config`.

### Ruido medido, y las nueve filas verificadas contra el binario

`error` + `warning` queda en **80**, idéntico al baseline. Los 14 `info` nuevos son
todos de tools sintéticas (`data_run_python` ×11 y tres más) y desaparecen con la
rebanada 3. Las dos entradas `mcp` no emiten nada porque no tienen bloques de campos.

Siguiendo la lección de la §21, la tabla de casos se **midió antes de escribir el
guard**, no después: se generó un grafo por cada fila y se corrió el binario. Las
nueve coinciden.

### Una mutación volvió a encontrar un test que pasaba por la razón equivocada

De cuatro mutaciones, dos no mataron nada al principio. El test que decía cubrir el
set unido usaba `subgraph.task` y `http_request.query_params`, y ninguno de los dos
aislaba lo que afirmaba: `subgraph` acepta cualquier clave, así que la regla nunca
llegaba a consultar los input ports; y `query_params` también está en
`config_fields`, así que la búsqueda en `reserved_input_keys` era irrelevante.
Borrar cualquiera de las dos mitades del set dejaba el test verde.

Los fixtures se cambiaron por los que sí aíslan: `add.a` es un input port de un nodo
de contrato **cerrado**, y `http_request.query_parameters` es la única clave del
catálogo que existe **sólo** en `reserved_input_keys`. Con ellos las cuatro
mutaciones muerden.

### La revisión encontró un fallback con costo invisible

`undeclared_key_policy` responde `AcceptsAnything` ante un placeholder que no
conoce — el silencio es el veredicto seguro, porque tratar al nodo como contrato
cerrado reportaría como inventada cada clave configurada. Lo que la revisión
señaló es que ese acierto tiene un costo que nadie vería: **una** clave placeholder
nueva en `config_fields` o `input_ports` apaga la regla entera para ese tipo de
nodo, en silencio.

No es drift hipotético: el catálogo ya usa otros cinco nombres (`<branch_name>`,
`<child_output>`, `<raw>`, `<raw_config>`, `<schema_fields>`), hoy sólo en
`output_ports`, que esta regla no lee. Cualquiera de ellos migrando lo alcanzaría.

El arreglo no fue cambiar el fallback —eso daría falsos positivos— sino convertir
la pérdida silenciosa en un test que falla: un guard recorre el catálogo y nombra
todo placeholder que la regla no maneje, explicando la consecuencia.

Una segunda pasada de revisión señaló que ese guard, tal como estaba, probaba
menos de lo que decía. **Pasaba gratis**: el catálogo shippeado está limpio, así
que nunca ejercitaba su propia capacidad de detectar; y su lista de placeholders
conocidos estaba **duplicada a mano**, de modo que borrar el brazo `<extra_keys>`
de la producción lo dejaba igual de verde — detectaba drift del documento, no del
código. Ambas cosas se arreglaron:

- El escaneo se extrajo a un helper parametrizado por catálogo, y un test lo corre
  contra un catálogo sintético con un placeholder inventado, comprobando que lo
  nombra. La verificación dejó de ser una edición manual del documento real que
  alguien tenía que acordarse de deshacer.
- `placeholder_policy()` pasó a ser la **fuente única de verdad**: la producción y
  el guard leen la misma función. Verificado por mutación — borrar el brazo
  `<extra_keys>` ahora hace fallar el guard con
  `does not know them: ["http_request.<extra_keys>"]`.

Un tercer test fija el fallback en sí, para que sea una decisión y no un accidente.

**Alcance.** Aditivo. Un `DiagnosticCode` nuevo, un mapa lateral en el catálogo, y
tres métodos públicos. Ningún grafo existente cambia de resultado en severidades
bloqueantes.

**Estado.** done.

---

## 23. El catálogo nombra los cinco tipos que sólo existen dentro de `tool_configurations`

**Qué.** Cierra los 14 `info` que la §22 dejó sobre tools sintéticas, dándole al
linter la información que le faltaba en vez de silenciarlo.

### Por qué no podían ir en `node_types`

Esa sección está cerrada en las dos direcciones contra el registry del motor: un
test falla si documenta algo que el motor no ejecuta, otro si el motor registra
algo sin documentar. Pero cinco nombres son válidos como `node_type` de una tool
sin ser nodos registrados — cuatro tools sintéticas que `llm_call` ensambla, más
`mcp`, que es un servidor remoto. Van en `tool_only_node_types`, aparte.

Sin esa lista el linter decía lo mismo de `data_run_python` —correcto, usado por
once grafos de este repo— y de `data_run_pythonn`, que no expone nada. Ahora
calla en el primero y en el segundo sugiere el nombre real, en vez de aconsejar
"agregá una entrada al catálogo", que para un typo manda al lugar equivocado.

Cada entrada declara además cómo se activa (`activated_by`): `map_key` para las
cuatro sintéticas, `node_type` para `mcp`. Ese hecho, verificado en el código, es
la base de una regla que llega en la rebanada siguiente. Un `activated_by`
desconocido cae en `map_key`, la lectura estricta — si cayera en la permisiva, esa
regla quedaría apagada en silencio para ese tipo. Un test lo fija.

### El guard de drift, y lo que NO cubre

El motor no enumeraba sus tools sintéticas en ningún lado: cada nombre es una
`pub const` en su propio módulo y la exposición son cuatro `if` sueltos en
`llm.rs`. Ahora hay una lista, `TOOL_ONLY_NODE_TYPES`, al lado de las tools, y un
test en infraestructura —la única capa que ve la lista y el catálogo a la vez— la
cruza en las dos direcciones. Verificado por mutación en ambos sentidos.

**Lo que no cubre**: una sexta tool olvidada en la lista *y* en el catálogo deja a
los dos coincidiendo en un conjunto incompleto, y el test pasa. Tener la lista en
un solo lugar reduce dos sitios a uno, pero no cierra eso; cerrarlo exigiría que
los `if` de `llm.rs` salieran de una tabla. Queda dicho en la doc del test, para
que un run verde no se lea como más de lo que promete.

### El defecto que la lista permite reportar: `TOOL_NEVER_EXPOSED`

Para las cuatro tools sintéticas el `node_type` de la entrada **es inerte**. Lo
que las activa es la **clave del mapa**: `llm_call` junta las claves en
`configured_aliases` y pregunta `configured_aliases.contains(TOOL_DATA_RUN_PYTHON)`.

```json
"mi_python":       { "node_type": "data_run_python" }    // no expone NADA
"data_run_python": { "node_type": "lo_que_sea" }         // sí expone la tool
```

Para cualquier otro nodo, poner un alias descriptivo es lo correcto y funciona.
La regla se invierte sólo para estas cuatro, y nada avisa: `available_tools`
busca el nombre en el registry, obtiene `None` y descarta la entrada **sin rama
`else` y sin log**.

Verificado corriendo dos grafos idénticos salvo la clave. El keyeado
`data_run_python` emitió `tool-input-start` y `tool-output-available`, y el
modelo llamó la tool con `{"code":"output = 2 + 2"}`. El keyeado `mi_python` no
emitió **ninguna** frame de tool y el agente contestó que no tenía herramienta.
Los dos salieron con código 0.

### Por qué esto no se pudo rebanar

Se intentó publicar la lista de nombres primero y la regla después. Una revisión
lo clasificó como `worsened` y tenía razón: enseñarle al linter estos cinco
nombres significa **saltearlos**, y saltearlos sin la regla convierte el caso de
arriba de un `NO_CATALOG_COVERAGE` débil a **silencio completo** — estrictamente
peor que antes de que el catálogo los conociera. La costura parecía limpia y no
lo era.

Van juntos, y un test fija la lección para que nadie vuelva a separarlos:
`teaching_the_linter_these_names_never_makes_a_broken_entry_quieter`.

### Ruido medido

`error` y `warning` quedan en 75 y 5, idénticos al baseline. Los `info` bajan de
**14 a 0**.

Un test existente se rompió al hacerlo, y con razón: usaba `data_run_python` como
ejemplo de "tipo sin cobertura", y ese nombre ahora es conocido. Habría seguido en
verde sin ejercitar la rama para la que se escribió.

**Alcance.** Aditivo. Una sección nueva en el catálogo, una lista de nombres, un
`DiagnosticCode` nuevo y dos métodos públicos. Ningún grafo existente cambia de
resultado en severidades bloqueantes.

**Estado.** done.

## 24. Las tools de un servidor MCP dejan de entrar al catálogo lazy

**Qué cambió.** Con `lazy_tool_loading` activo, cada tool que un servidor MCP exponía
recibía además una línea en el catálogo lazy. Ya no: quedan sólo en `tools`, siempre
presentes, con el schema que publicó el servidor.

**Por qué era un defecto.** El catálogo es lo que `describe_tool` revela a demanda, y
`describe_tool` resuelve contra `lookup_for_describe`, que guarda `ToolConfiguration`s.
Una tool MCP no tiene una —es un `ToolDefinition` que llegó del servidor—, así que podía
listarse pero nunca describirse. Y estar en el catálogo la **escondía** de `tools[]` hasta
ser "descubierta". En un grafo cuyas únicas tools son MCP el resultado era: el modelo
recibía un `describe_tool` sin handler detrás, lo llamaba, y le volvía `Tool not found`.

**Se curaba solo, por accidente.** `reconstruct_discovered_set` lee las **llamadas** del
asistente, no sus resultados, así que un `describe_tool(name = X)` fallido igual marcaba X
como descubierta y en la iteración siguiente la tool aparecía y funcionaba. Por eso el E2E
terminaba bien: costaba dos o tres turnos y dejaba errores confusos en el contexto del
modelo, no una falla visible.

**Por qué no se arregló al revés.** La opción aparentemente obvia —dejar de anunciar
`describe_tool` cuando no hay nada describible— **rompe la feature**: las tools MCP
seguirían en el catálogo, ocultas detrás de un descubrimiento que ya nunca podría ocurrir,
y pasarían a ser no invocables.

**El costo, explícito.** Un servidor con muchas tools ahora manda todos sus schemas cada
turno, que es justo lo que lazy existe para evitar (tope de 64 tools por servidor, hasta
32 KB de schema cada una). Volverlas lazy de verdad exige enseñarle a `describe_tool` a
responder desde un `ToolDefinition`; queda como trabajo aparte, no como olvido.

**Alcance.** Sin cambio de API. Un grafo que mezcla tools lazy normales con MCP sigue
igual: las primeras se catalogan y describen, las MCP quedan en el set siempre-presente.
Sin `lazy_tool_loading` nada cambia. ADP no afectado.

**Estado.** done.

## 25. El módulo MCP deja de ser mudo (entrada retroactiva)

**Qué cambió.** Tres PRs (#265, #270, #271) instrumentaron las tres rutas donde MCP
puede degradarse. Se registran acá en conjunto porque ninguna dejó entrada propia
—un descuido contra la regla de que la documentación viaja con el código—, y el
comportamiento ya está en `develop`.

**Antes.** El módulo emitía cuatro líneas sin nombre de evento y ninguna latencia.
Con un servidor de un tercero lento o caído, la única señal era un conteo agregado
*después* del hecho: no se podía saber **cuál** servidor falló, **por qué**, ni
**cuánto tardó**. El pool era directamente invisible.

**Ahora,** siguiendo la convención del repo (`target: "colmena::mcp"`,
`event = "mcp.<acción>"`, campos, mensaje humano):

- **Fetch/connect por servidor** (#265): `mcp.server_ready`, `mcp.server_unavailable`
  con un `reason` estable (`prepare` / `tools_list` / `no_result`) y `ms`.
- **Despacho** (#270): `ms` y `tool_call_id` en `mcp.dispatch_ok` /
  `mcp.dispatch_failed`, más `mcp.dispatch_refused_secret` donde el motor bloquea un
  secreto antes de la red.
- **Pool** (#271): `connection_reused` / `connection_opened` / `connection_cooldown` y
  `catalog_hit` / `catalog_miss`, más `pool_evicted`.

**Tres decisiones que valen más que la lista.** `reason` es una etiqueta estable y no
el texto de error, para que se pueda agrupar. `no_result` **omite** `ms` en vez de
inventar un `0` que un dashboard leería como "resolvió instantáneamente". Y
`connection_reused` / `catalog_hit` se emiten desde dos sitios cada uno —el fast path
y el re-check tras el lock—, distinguidos por `raced`, para que la contención del
single-flight se vea en vez de esconderse en una línea indistinguible.

**Privacidad.** Nunca se loguea el valor de un header, el cuerpo de un resultado, ni
los argumentos de una llamada. Sí: alias, host (sin scheme, path, query ni userinfo),
conteos, tamaños, tiempos y la `key` del pool, que es un digest derivado de
referencias y jamás de un secreto resuelto.

**Alcance.** Instrumentación pura: ningún cambio de comportamiento, de scope de lock
ni de semántica de degradación. ADP no afectado. Referencia completa en
[`developer_guide/52`](developer_guide/52_mcp_observability.md), que ahora incluye
además cómo atribuir el costo en contexto de cada servidor y la postura de seguridad
vigente.

---

## 26. El linter dice, antes de correr, lo que el motor sólo decía al cargar

**Qué.** Cierra los dos huecos que quedaban del track del linter, y los dos son
la misma clase de problema: un diagnóstico que existía pero llegaba tarde, o un
consejo que no se podía seguir.

### `MALFORMED_TOOL_ENTRY`

`Graph::validate()` rechaza el grafo entero cuando el `node_schema` de una tool
no se puede leer, y desde el §18 esa validación corre en **toda** entrada del
motor. El diagnóstico ya existía; lo que faltaba era decirlo antes de ejecutar,
que es para lo que existe un linter.

Son **dos** familias de rechazo, y hacen falta las dos. La primera es el bloque que
no deserializa a `NodeSchema`: `null`, un escalar, un array, o un objeto alguna de
cuyas entradas no es una definición de campo válida. La segunda es el bloque que
deserializa bien pero `parse_node_schema` rechaza: un campo visible al LLM sin
`type`, o uno `array` sin `items` / `items.type`. La segunda no es un detalle:
`{ "body": { "required": true } }` es un `NodeSchemaField` impecable, así que la
primera familia no lo ve y el motor igual lo rechaza.

La regla lo dice **llamando a las mismas funciones** que usa `validate()`:
deserializar a `NodeSchema` y después `parse_node_schema`. Las dos viven en el
dominio, igual que el linter, así que compartirlas no cruza ninguna frontera —
y hace la divergencia imposible por construcción, que es mejor que un test que
la vigile.

Verificado forma por forma contra el binario. Las nueve que el motor rechaza —seis
de la primera familia, tres de la segunda— las reporta el linter; las dos que acepta
—`node_schema` ausente y uno bien formado— no. Cero disparos sobre el corpus, que
queda en 75 y 5 como el baseline.

**Reportarlo silencia la regla de precedencia para esa entrada**, y sólo esa. El
consejo de `DEAD_FIXED_CONFIG` —mover las claves dentro de ese mismo
`node_schema`— dejaría el grafo igual de inejecutable, porque el schema es
justamente lo que está roto. Un test de la §23 fijaba deliberadamente esa
imprecisión mientras su propio docstring nombraba el remedio; ese test se borró
en vez de dejarlo contradiciendo al nuevo.

Las **reglas de campos no se silencian**: son defectos independientes. Una entrada
con un `node_schema` malformado *y* una clave inventada en `fixed_config` reporta
las dos cosas. Esconder la segunda costaba un viaje de ida y vuelta extra por algo
que no tenía relación con la primera.

### El mensaje nombra formas y claves, nunca un valor

Reenviar el error de serde imprime el string ofensor **literalmente**
(`Unexpected::Str`). Así que la forma más probable de cometer este error —poner un
valor directo bajo la clave en vez de dentro de una definición de campo— copiaba
ese valor a stdout, al reporte `--format json` y a los bindings. El caso típico no
es inocente: `"node_schema": { "api_key": "sk-live-…" }` publicaba la credencial en
el log de CI, que es justamente donde el cuerpo del grafo NO se imprime.

`unreadable_schema_shape` describe la forma en su lugar —`` `api_key` es un
string ``, `` `body` es un objeto pero no una definición de campo válida ``, `el
bloque entero es un array``— con las claves ofensoras **ordenadas**, para que la
oración no dependa del orden en que el autor las escribió. Los errores de
`parse_node_schema` se reenvían sin tocar porque nombran la etiqueta del campo y
nunca su valor.

Una excepción que conviene nombrar en vez de fingir que no existe:
`INVALID_FIELD_VALUE` **sí** imprime un valor, junto a la lista de aceptados. Pero
sólo para un campo que el catálogo declara con `valid_values` —un enum cerrado como
`method`—, donde no aterriza una credencial. La distinción que importa no es "valor
sí / valor no", sino si el lugar es un enum documentado o una clave libre que nombra
el autor.

### Y el mensaje es el mismo en cada corrida

`parse_node_schema` recorre un `HashMap` y devuelve el **primer** campo que le
disgusta, así que con dos campos malos el motivo impreso dependía del seed del
proceso. Tolerable para un crash de carga; no para un reporte que se diffea en CI.
`first_parse_rejection` le pregunta de a un campo por vez en orden alfabético y se
queda con la primera queja. Misma función, mismo veredicto, oración estable.

Le pasa el esquema entero al final como **guarda contra deriva**, no porque atrape
algo hoy: todos los errores de `parse_node_schema` viven en su bucle por campo, y su
segunda pasada no tiene rama de error, así que un esquema cuyos campos pasan uno a
uno no puede fallar entero. Verificado contra el binario con el caso que se había
citado como contraejemplo —dos contenedores compartiendo una clave hija—: sin
hallazgo. La justificación original de esa línea era falsa.

### El consejo que no se podía seguir

Un tipo tool-only puesto como `type` de un nodo del grafo se reportaba como
`NO_CATALOG_COVERAGE` (info) aconsejando agregar una entrada al catálogo. Es
imposible: `node_types` está cerrado en ambas direcciones contra el registry y esa
entrada haría fallar el test suite.

Ahora el consejo dice dónde va el nombre. La severidad depende del contexto: con
`from_catalog` —lo que usa el CLI— pasa a `UNKNOWN_NODE_TYPE` (error, ese grafo no
corre); con `with_embedded_catalog`, que no saca conclusiones sobre tipos de nodo,
sigue siendo `NO_CATALOG_COVERAGE` (info) y sólo cambia el texto. Un tipo realmente
desconocido conserva el consejo original; un test fija las dos ramas.

### Verificación

2771 tests, clippy limpio, links limpios. Corpus `error=75 warning=5 info=0`,
idéntico al baseline.

**Once mutaciones**, corridas contra los bytes que se publican y no contra una
versión anterior. Las once matan un test: descartar el error de
`parse_node_schema`, quitar la supresión de `DEAD_FIXED_CONFIG`, no distinguir el
tipo tool-only en `lint_node`, volver a suprimir las reglas de campos, perder el
consejo en el contexto `Unchecked`, imprimir el valor en vez de su forma, listar las
claves ofensoras sin ordenar, truncar el bucle de sondeo a un solo campo, quitar su
`keys.sort()`, preguntarle a `parse_node_schema` por el esquema entero, y hacer que
la rama de objeto imprima el valor que encontró.

Las últimas tres **no mataban siempre** cuando se midieron por primera vez, y eso
fue el hallazgo útil: las tres dejan que el orden de un `HashMap` elija cuál de los
cuatro campos malos del fixture se nombra, así que sobrevivían cuando la suerte les
daba el correcto. Medidas: 18 de 20, 11 de 12, y una que había pasado por
determinística sobre **una sola corrida**. El guard de estabilidad ahora repite el
bloque veinte veces, y cada `lint_json` construye un mapa nuevo con un `RandomState`
nuevo, así que una implementación rota tiene ~4⁻²⁰ de chance mientras la ordenada es
invariante. Re-medidas con la repetición: 15 de 15 las tres.

La última salió de una revisión y vale la pena por qué: la guarda de la fuga cubría
**una de las dos ramas**. Su fixture tenía solo claves ofensoras no-objeto, así que
la rama que reporta `` `creds` es un objeto pero no una definición de campo válida ``
podía imprimir el valor y los 71 tests seguían en verde. Y esa es justamente la rama
que una credencial alcanza primero: `{"creds": {"required": "sk-live-…", "type": 5}}`
parece una definición de campo y falla solo porque una clave interna tiene el tipo
equivocado. El fixture ahora lleva una fila así, con un segundo secreto un nivel
más abajo.

Dos detalles de los fixtures son el guard, no decoración. El de la fuga va escrito
**fuera** de orden alfabético: con las claves ya ordenadas en el documento, quitar
el `sort()` no cambia nada y el test pasa sobre una implementación rota. El de la
estabilidad pone un campo **válido** en la primera posición alfabética, para que el
bucle tenga que avanzar; con un campo malo ahí, truncarlo a una iteración pasaba
igual.

**Alcance.** Aditivo. Un `DiagnosticCode` nuevo y dos ramas de mensaje. Ningún
grafo del corpus cambia de resultado.

**Estado.** done.

## 27. Allowlist de hosts para MCP, opcional y apagada por default

**Qué cambió.** `COLMENA_MCP_ALLOWED_HOSTS` es una nueva variable de entorno,
opcional, que restringe a qué hosts puede conectarse un servidor MCP declarado
en un grafo. Acepta una lista separada por comas (`host.a,host.b`); cada
entrada se recorta y se compara en minúsculas. El match es sobre **hostname
únicamente** — el puerto se ignora, así que `host.a` en la lista permite tanto
`https://host.a/x` como `https://host.a:8443/x` — y es **exacto**: sin
wildcards, sin coincidencia de subdominio. `example.com` en la lista NO cubre
`sub.example.com`, y una entrada `*.example.com` es un string literal que nunca
matchea nada real — un matcher permisivo da falsa confianza y por eso se optó
por uno estricto, documentado como tal.

**Fix de un bypass SSRF verificado antes de mergear.** La primera versión de
este módulo traía un parser de URLs hand-rolled (`host_of`) que partía la
autoridad solo en `['/', '?', '#']` — nunca en `\`. El cliente HTTP real
(`rmcp` sobre `reqwest`, cuyo tipo `Url` es el crate `url`, compatible con
WHATWG) SÍ trata una barra invertida como terminador de autoridad para
esquemas especiales como `https`. Con eso,
`https://evil.internal\@allowed.example.com/mcp` era leído por `host_of` como
host `allowed.example.com` (aprobado) mientras `reqwest` conectaba en realidad
a `evil.internal`: un bypass completo del control. La corrección no fue
agregar `\` al set de delimitadores — punycode, normalización y otras reglas
WHATWG seguirían pudiendo disentir de un segundo parser hand-rolled — sino
tener una única noción de "host", derivada del mismo parser que decide a dónde
conecta el cliente. Ver más abajo.

**Por qué.** Hoy cualquier URL HTTPS que un grafo declare para un servidor MCP
es alcanzable, incluidos endpoints internos: es superficie SSRF, y la decide
quien escribe el grafo, no quien opera la instancia. Esto le da al operador un
control que hoy no existe, sin tocar los grafos que ya corren.

**Garantía de compatibilidad.** Vacía o sin definir (el default) permite
cualquier host — el comportamiento de hoy no cambia en absoluto hasta que un
operador fija la variable explícitamente. Los tests de degradación existentes
en `wire.rs` (servidor inalcanzable, credencial que no resuelve) no se
tocaron.

**Orden deliberado.** El chequeo corre en el fan-out de `wire()`, ANTES de
llamar a `bind()`. Eso significa que un host rechazado **nunca** causa que se
resuelva o se descifre una credencial — un secreto configurado para un
servidor cuyo host está fuera de la allowlist ni siquiera se toca.

**Degrada, no falla.** Un host rechazado se comporta exactamente como un
servidor inalcanzable: la nueva variante `FetchFailure::NotAllowed` (etiqueta
`not_allowed`) hace que el alias caiga en `unavailable`, el modelo sea
notificado en el mensaje de sistema, y el turno siga con el resto de las
tools. Nunca hace fallar el `llm_call` ni el grafo. Se reporta con
`mcp.server_unavailable` y `reason = "not_allowed"`.

**Qué NO resuelve.** La allowlist acota A DÓNDE puede ir el tráfico MCP, no
QUÉ va en él — nada inspecciona los argumentos salientes más allá del rechazo
de secure values que ya existía. El problema del *confused deputy* sigue
abierto: si el modelo decide mandarle a un host permitido algo que tenía en
contexto, sale igual.

**Implementación.** Módulo `mcp/allowlist.rs` con `parse_allowlist` (pura) +
`url_is_allowed` (pura, la decisión de seguridad — toma la URL completa, nunca
un host ya extraído, para que ningún call site pueda pasarle el host de un
parser distinto) + `allowed_hosts_from_env` (el thin wrapper que lee
`std::env`, siguiendo el mismo patrón función-pura/wrapper-de-env que
`pool_size_from` en `mcp_registry/mod.rs`). El host que decide viene de
`dialed_host`, que llama a `reqwest::Url::parse` — el mismo parser que usa el
transporte HTTP real para conectar — y devuelve `None` en vez de un valor
best-effort cuando la URL no parsea; `url_is_allowed` trata ese `None` como
rechazo (fail closed) cuando hay allowlist configurada. Para el log line
existe `host_for_log`, separado deliberadamente y documentado como NO apto
para decisiones de seguridad — mezclar ambos roles en un único helper (el
extinto `host_of`) fue exactamente la causa raíz del bypass. Sin cambio de API
pública; ADP no afectado.

**Estado.** done.

---

## 28. El linter entra en el `target` de un `for_each`

**Qué.** Primera mitad de L2b. Un `for_each` embebe `{node_type, node_schema}` —la
misma forma que una entrada de tool— y lo despacha una vez por fila. Ese bloque nombra
campos de otro tipo de nodo y **ninguna regla lo miraba**, así que el defecto de la §20
(`url` donde `http_request` lee `base_url`) seguía invisible un contenedor más abajo.

Ahora se revisa con la regla que ya existía para los campos de una tool: misma
pregunta, misma severidad, mismas palabras. La comprobación se extrajo a
`check_configured_keys` y la usan los dos llamadores, así que no pueden divergir.

**Tres puertas, no una.** El nodo resuelve su `target` con `cfg_or_input`, así que el
bloque llega por el `config` del propio nodo o —cuando el `for_each` está expuesto como
tool— por `node_schema.target.fixed` o `fixed_config.target` de la entrada. Las tres se
recorren.

### Verificación

**E2E contra el servicio real** (`httpbin.org`), capturado en
`/tmp/colmena_e2e/e2e_a_url_query.sse`. Un `for_each` de dos filas cuyo target declara
`url` junto a un `base_url`/`endpoint` correctos: httpbin devuelve

```
"args":{"row_id":"1","url":"https://not-where-it-goes.example"}
"args":{"row_id":"2","url":"https://not-where-it-goes.example"}
```

O sea que `url` **se fue como query param**, que es exactamente lo que dice el mensaje
de `REPURPOSED_TOOL_FIELD`. La afirmación del diagnóstico está medida, no deducida.

Corpus `error=75 warning=5 info=0`, idéntico al baseline, con **cero** hallazgos de
`target`. Ese cero se comprobó que significa "los grafos están bien" y no "el walker
está ciego": el corpus tiene 4 targets reales (1 como nodo del grafo, 3 por
`node_schema.target.fixed`), e **inyectando una clave inventada en dos de esos archivos
reales** la regla los reporta por las dos puertas. La tercera (`fixed_config.target`) no
tiene ningún caso en el corpus, así que la sostiene sólo su test — dicho en su docstring.

**Cuatro mutaciones**, todas matan un test: quitar la puerta del nodo, quitar
`node_schema.target.fixed`, quitar `fixed_config.target`, y quitar el guard de "el nodo
declara esta clave" — esta última prueba que el control negativo (un `base_url` correcto
no se reporta) no pasa por vacío.

### Lo que queda para la otra mitad

Reportar un `target.node_schema` **malformado** se rebanó aparte por el cap de 500
líneas (L2b-bis). Omitirlo no deja nada más callado que hoy, así que la costura es
segura — a diferencia de la de la §23, donde saltear los nombres sin la regla dejaba un
caso roto en silencio total.

Esa mitad además trae una corrección: la primera versión de este trabajo afirmaba, en el
diagnóstico y en la guía, que un target malformado hacía despachar las filas **sin
validar**, razonando desde el par `if let Ok(...)` sin rama else de `for_each.rs`.
**Correrlo lo desmintió**: cada fila falla con `Invalid node_schema: …` porque
`merge_args_into_schema` corre esas mismas dos comprobaciones antes. Verificado para las
dos familias de rechazo. El error importaba en la peor dirección —mandaba a buscar filas
corruptas que no existen— y de paso reescribió L2c, que no es una falla silenciosa sino
una guarda inalcanzable.

**Alcance.** Aditivo: ninguna regla nueva, un contenedor nuevo para una que ya existía.
Ningún grafo del corpus cambia de veredicto → ADP no afectado.

**Estado.** done.

---

## 29. Un `target` malformado se reporta antes de correr, con el costo que de verdad tiene

**Qué.** Segunda mitad de L2b, rebanada aparte de la §28 por el cap de 500 líneas. Un
`target.node_schema` que el motor no puede parsear ahora se reporta como
`MALFORMED_TOOL_ENTRY`, con las dos familias de rechazo: el bloque que no deserializa a
`NodeSchema` (`"body": "not-an-object"`) y el que deserializa bien y `parse_node_schema`
rechaza igual (un campo visible al LLM sin `type`).

Reusa `node_schema_rejection`, la misma función que ya reproduce la compuerta de
`Graph::validate()` para una entrada de tool. Lo que **no** reusa es el mensaje.

### La afirmación que la corrida desmintió

La primera versión de este trabajo decía —en el diagnóstico, en la guía §51 y en el
cuerpo del PR— que un `target` malformado hacía despachar las filas **sin validar**. Lo
había deducido del par `if let Ok(...)` sin rama else que hay en `for_each.rs`, que
parece saltearse la validación de params requeridos por fila.

**Correrlo lo desmintió.** `merge_args_into_schema` corre **esas mismas dos
comprobaciones** unas líneas antes y falla la fila. Medido, con control de dos lados:

| Grafo | total/ok/err | Error por fila | Requests al servicio |
|---|---|---|---|
| Control: schema válido, requerido que nadie aporta | 2/0/2 | `missing required param 'must_be_present'` | 0 |
| Familia 1: `"body": "not-an-object"` | 2/0/2 | `Invalid node_schema: invalid type: string` | 0 |
| Familia 2: campo sin `type` | 2/0/2 | `Invalid node_schema: … is LLM-visible but missing type` | 0 |

Capturas en `/tmp/colmena_e2e/e2e_{b,c,d}_*.sse`. Sobre los mismos dos grafos el linter
reporta `MALFORMED_TOOL_ENTRY` **sin correr nada**, que es el punto.

El mensaje ahora dice lo que pasa: el grafo **carga**, y el lote muere a mitad de la
corrida en vez de antes. Sigue siendo razón para lintearlo —es el viaje que se ahorra—
pero es la razón verdadera. El error apuntaba en la peor dirección: mandaba a un
operador a buscar filas corruptas que no existen.

**Y arrastró un segundo defecto.** Al rebanar la §28 corté la regla pero **no revisé la
fila de la tabla de códigos**, que quedó en `develop` describiendo un comportamiento que
esa rebanada no shipeó. Corregida acá. La lección no es "revisá la tabla": es que
rebanar deja referencias colgando en documentos que uno no está mirando, y hay que
barrerlos igual que se barren los usos de un símbolo.

### Verificación

**Tres mutaciones**, todas matan un test: no reportar nunca, cegar la familia 2, y
reportar todos los target en vez de sólo los rechazados. La segunda hubo que
reformularla: la versión obvia dejaba `first_parse_rejection` sin usar y, con
`warnings = "deny"`, fallaba la **compilación** en vez del test — una mutación que no
compila no prueba nada.

Corpus `error=75 warning=5 info=0`, sin cambios.

**Alcance.** Aditivo. Ningún grafo del corpus cambia de veredicto → ADP no afectado.

**Estado.** done.

---

## 30. Un catálogo de ejemplos rotos (tanda 1: los de nivel de nodo)

**Qué.** Diez grafos deliberadamente rotos en
[`tests/lint_examples/`](../tests/lint_examples), uno por cada diagnóstico **a nivel de
nodo** más un control limpio, documentados en la guía §51 **con la salida real del
binario** — generada, no transcrita. Un test falla si un ejemplo deja de producir el
diagnóstico que la guía muestra.

Viven **fuera** de `tests/graphs/`. Ese árbol es el corpus sobre el que se mide el ruido
(`error=75 warning=5` sobre 303 grafos realistas), y meterle archivos rotos a propósito
envenenaría justo el número que dice si la herramienta vale la pena escuchar.

Los de `tool_configurations`, los del `target` de un `for_each` y el punto ciego del
`subgraph` inline llegan en la tanda 2, junto con el test que exige que ningún
`DiagnosticCode` se quede sin ejemplo. Ese test viaja con la tanda que **completa** la
cobertura: ponerlo acá sería prometer una garantía que esta tanda no puede cumplir.

### Lo que el ejercicio encontró

Escribir los ejemplos falsificó tres cosas que yo había dado por buenas, y ese fue el
valor real:

- **`add` ignora su `config`.** El ejemplo de `UNKNOWN_NODE_PROPERTY` traía
  `{"left": 1, "right": 10}` y salieron dos errores extra. No era un falso positivo:
  `execute` recibe `_config` y lee sus inputs `a`/`b`. El ejemplo estaba mal, no la
  herramienta.
- **`trigger` no es un tipo de nodo.** El motor registra `trigger_webhook`. Mi grafo de
  control "limpio" no lo estaba.
- **`router.mode` y `router.branches` no son lo que asumí.** El linter me corrigió
  mientras yo escribía el ejemplo que iba a ilustrar otra cosa.

Los tres son la razón por la que los ejemplos se **corren** en vez de escribirse: un
catálogo hecho de memoria documenta lo que uno cree, no lo que pasa.

### Y un hallazgo que no se arregla acá

`"type": "trigger"` sale como `NO_CATALOG_COVERAGE` (**info**) aconsejando agregar una
entrada al catálogo — cuando ese grafo no arranca. La justificación de esa debilidad era
que un tipo ausente del catálogo podría estar registrado sin documentar; desde que
`node_types` quedó cerrado en ambas direcciones contra el registry eso **ya no puede
pasar**. Subirlo a error cambia qué falla bajo `--strict`, así que va aparte: anotado
como **L11** en el [BACKLOG](BACKLOG.md).

### Verificación

Los diez ejemplos se corren por el binario real y su salida es la que la guía muestra.
El control además se pasó **por el motor**, no sólo por el linter: contra OpenAI termina
en `[DONE]` sin errores, `promptTokens: 842`, `completionTokens: 20`
(`/tmp/colmena_e2e/lint_example_18_clean.sse`). Un control que lintea limpio pero no
arranca diría que el linter calla, no que tiene razón.

Los otros nueve son grafos rotos a propósito: correrlos por el motor no agrega evidencia
—fallarían, que es el punto— así que la corrida que importa para ellos es la del linter,
capturada en la guía.

**Alcance.** Sin cambios de código: fixtures, tests y documentación. El corpus sigue en
`error=75 warning=5 info=0` — los ejemplos no lo tocan, que es el punto de dónde viven.

**Estado.** done.

---

## 31. El catálogo de ejemplos, completo (tanda 2)

**Qué.** Los ocho ejemplos que faltaban: los cuatro diagnósticos de
`tool_configurations`, los dos del `target` de un `for_each`, el punto ciego del
`subgraph` inline y uno compuesto que muestra el orden del reporte. Con esto el catálogo
llega a dieciocho y cubre **los doce** `DiagnosticCode`.

Llega también el test que lo exige: `every_diagnostic_code_the_linter_can_emit_has_an_example`.
Viajó con esta tanda a propósito — ponerlo en la §30 habría sido prometer una garantía
que aquella tanda no podía cumplir, y la única forma de hacerlo pasar habría sido
debilitarlo.

### El ejemplo del punto ciego, y lo que su test no puede

El ejemplo 16 mete tres defectos dentro de un `child_graph_inline` —un `modle`, un campo
inventado y un edge colgado— y su expectativa es **ningún hallazgo**. Eso falla el día
que L2 cierre, verificado subiendo uno de esos defectos al nivel superior: el test se
pone en rojo.

Lo que **no** puede atrapar es que ese fixture deje de estar roto: reparar un defecto
adentro del hijo inline no cambia nada que el linter pueda ver, así que la expectativa
sigue valiendo. Es inherente —el punto del ejemplo es justamente que nadie mira ahí— y
por eso lleva tres defectos en vez de uno. Está dicho en el test y en la guía en lugar de
dejarlo como un agujero que alguien descubra más tarde.

Vale la pena nombrarlo porque la primera mutación que probé fue justamente ésa —reparar
un defecto interno— y **pasó**. Una mutación que pasa no siempre acusa un test flojo: a
veces acusa una mutación mal elegida. La distinción se resuelve mirando qué puede ver el
código bajo prueba, no repitiendo la mutación.

### Verificación

**Tres mutaciones**: un código pierde su único ejemplo → rojo; reparar el defecto del
ejemplo 10 → rojo; el punto ciego deja de estar en silencio → rojo. La cuarta (reparar un
defecto **interno** del ejemplo 16) pasa, por lo de arriba.

2803 tests, corpus `error=75 warning=5 info=0` sin cambios — los ejemplos siguen fuera de
`tests/graphs/`, que es el punto de dónde viven.

**Alcance.** Sin cambios de código: fixtures, tests y documentación.

**Estado.** done.

---

## 32. El linter entra en un `subgraph` inline

**Qué.** Cierra **L2**, la última clase entera de grafos que el linter no revisaba. Un
`child_graph_inline` es un documento de grafo completo —el motor se lo pasa tal cual al
ejecutor hijo, que lo deserializa como `Graph` y lo valida—, pero ninguna regla entraba:
todos los walkers leen los `nodes.*` de *ese* documento y paran ahí. Un `modle`, un campo
inventado y un edge colgado adentro de un hijo linteaban limpio.

Ahora el hijo pasa por **el mismo set de reglas**, no por un subconjunto elegido a mano.
Para eso se extrajo `lint_document`: cualquier cosa cierta de un grafo de primer nivel lo
es de un hijo, y elegir a mano cuáles reglas aplicar habría sido una decisión que envejece
sola.

Cada hallazgo se atribuye al **path que lo alcanza** (`nested/chat`), con `/` —el
separador que el motor reserva para calificar paths de subgrafo y que `Graph::validate()`
rechaza en ids de autor, así que un id con prefijo no puede chocar con uno real—. Un
hallazgo sobre el grafo y no sobre un nodo usa el path solo, que es lo único que dice qué
subgrafo abrir.

Dos puertas, igual que el `target` de un `for_each`. Sin tope de recursión: un hijo inline
es JSON literal, no hay ciclo ni profundidad que acotar. `child_graph_path` **no** se
sigue: leer un archivo hermano haría que la respuesta dependa del sistema de archivos.

### Corrido contra el motor, no supuesto

Un hijo inline con un edge cuyo origen no existe: el linter lo reporta, y el motor
**termina en `[DONE]` con exit 0 y cero errores** — mientras el nodo `sum` de ese hijo
nunca produce salida, que ni siquiera aparece en el resultado final. Es la tesis de
`EDGE_UNKNOWN_NODE` demostrada un nivel más abajo: el grafo hace silenciosamente menos de
lo que dice, y nadie se entera. Captura en
`/tmp/colmena_e2e/inline_child_dangling_edge.sse`.

### Lo que destapó, y cómo se comprobó que el cero es un cero

Sobre el corpus: **33 hijos inline en 20 archivos, 80 nodos hijos** que nadie revisaba.
Hallazgos nuevos: **cero** — están bien escritos.

Ese cero se comprobó inyectando un campo inventado en un nodo hijo de dos archivos
**reales**: la regla lo reporta por las dos puertas (`inner_subgraph/ask_user`,
`analista/equipo_calculo/calculista`).

**El primer intento dio "perdido", y era la mutación.** Había caído en un nodo `input`,
que acepta cualquier clave por diseño, así que no había nada que reportar. Es la segunda
vez en este track que una mutación mal elegida se disfraza de regla rota; la forma de
distinguirlas sigue siendo mirar qué puede ver el código bajo prueba.

### El guard del catálogo hizo lo suyo

El ejemplo 16 documentaba este punto ciego con la expectativa «ningún hallazgo», y su
comentario decía que fallaría el día que L2 cerrara. **Falló solo**, nombrando
`{"EDGE_UNKNOWN_NODE", "UNKNOWN_FIELD"}`. Se actualizó la expectativa, se reescribió el
comentario y se renombró el archivo, que decía `blind_spot` sobre algo que ya no lo es.

**Alcance.** Aditivo: ninguna regla nueva, un contenedor nuevo para todas. Ningún grafo
del corpus cambia de veredicto → ADP no afectado.

**Estado.** done.

---

## 33. La guarda de `for_each` deja de invitar a la lectura equivocada

**Qué.** Cierra **L2c**. El nodo validaba los params requeridos de cada fila dentro de un
par de `if let Ok(...)` **sin rama else**, que se lee como "si el schema no parsea,
salteá el chequeo y seguí". Pasa a propagar con `?`.

Ese item nació afirmando que ahí había una falla silenciosa; la §29 lo corrigió con una
corrida (`merge_args_into_schema` hace las mismas dos comprobaciones unas líneas antes y
falla la fila primero, así que la guarda era inalcanzable). Lo que quedaba era una forma
que convenció a un lector —yo— de documentar un defecto que no existía, y que iba a
volver a convencer al siguiente.

### El refactor no es cosmético: se midió

| Con la comprobación de `merge_args_into_schema` **cegada** | Resultado |
|---|---|
| Forma vieja (`if let Ok`, sin else) | `err=0` — **la fila despacha sin validar** |
| Forma nueva (`?`) | la fila falla igual, con `Invalid node_schema` |

O sea que la redundancia dejó de ser código muerto y pasó a ser **defensa en
profundidad**: el chequeo por fila ya no depende de que el merge siga haciendo el suyo.
Un punto único de fallo menos.

La primera vez que intenté esta medición el script falló a mitad y el test corrió sobre
código **sin mutar**, dando un "ok" que no medía nada. Se rehízo tomando el archivo de
`HEAD` —que ya tiene la forma vieja— en vez de reconstruirla con reemplazos de texto.
Una mutación a medio aplicar es indistinguible de una que no mata.

**Verificación.** Test nuevo que fija la invariante (`a_target_schema_that_cannot_be_parsed_fails_the_row`)
más las 28 pruebas de `for_each` en verde. Sin cambio de comportamiento observable.

**Alcance.** Refactor. Ningún cambio de API ni de salida → ADP no afectado.

**Estado.** done.

---

## 34. Un tipo ausente del catálogo pasa a ser un error

**Qué.** Cierra **L11**. Con sólo el catálogo en mano, un tipo de nodo sin entrada se
reportaba como `NO_CATALOG_COVERAGE` (**info**) aconsejando agregar una entrada a
`docs/node_configurations.json`. Ahora es `UNKNOWN_NODE_TYPE` (**error**).

### Por qué la duda dejó de estar justificada

La justificación original era buena: afirmar «el motor no puede correr ese tipo» con
sólo el catálogo es **falso para un nodo registrado pero todavía no documentado**. Eso
era cierto cuando se escribió.

Dejó de serlo cuando `node_types` quedó cerrado **en ambas direcciones** contra el
registry. Un tipo registrado sin documentar hace fallar
`every_registered_node_type_is_documented_in_the_catalog`; uno documentado que el motor
no registra hace fallar `the_catalog_documents_no_node_type_the_engine_cannot_run` — y
el harness arma el registry **con** sus cuatro nodos condicionales, así que el conjunto
bajo prueba no se encoge en silencio. En cualquier build cuya suite pase, el catálogo es
espejo del registry, y la ausencia **prueba** que el grafo no arranca.

Lo que costaba la duda era concreto: `"type": "trigger"` —el motor registra
`trigger_webhook`— fue uno de los siete defectos reales que destapó la §20, y salía como
una nota aconsejando una entrada de catálogo que **agregarla habría roto el test suite**.

### El riesgo se midió antes de tomarlo

Subir infos a errores cambia qué falla bajo `--strict`, así que la pregunta era cuánto.
Medido sobre los 303 grafos: `NO_CATALOG_COVERAGE` a nivel de nodo dispara **cero
veces**. El corpus queda idéntico —`error=75 warning=5 info=0`— y nada que hoy pase
`--strict` deja de pasarlo.

### Lo que NO cambió

- El brazo `Unchecked` (`with_embedded_catalog`) sigue sin opinar sobre tipos de nodo:
  existe para eso, y el catálogo de un build no dice nada de un motor por el que no se
  preguntó.
- El `node_type` de una **tool** sigue reportando falta de cobertura. Ese brazo no se
  toca acá, y es el que mantiene `NO_CATALOG_COVERAGE` alcanzable — el ejemplo 08 del
  catálogo se reapuntó a esa forma.

### Tres tests reconciliados, no silenciados

Dos tests preexistentes afirmaban el contrato viejo **con su razón escrita en el
docstring**. Se reescribieron explicando por qué esa premisa murió, en vez de dar vuelta
la aserción y seguir.

El tercero era mío y estaba mal: afirmaba que el consejo no debía mencionar
`node_configurations.json`. **Leer** ese archivo es un consejo perfectamente seguible;
lo inseguible era **agregarle** una entrada. La aserción ahora dice eso.

**Alcance.** Cambia la severidad de un diagnóstico bajo `CatalogOnly`. Ningún grafo del
corpus cambia de veredicto → ADP no afectado.

---

## 35. Dos secciones no pueden compartir número, y ahora CI lo sabe

**Qué.** Las §26 estaban **duplicadas** en este mismo archivo: dos PRs mergeados el mismo
día agregaron cada uno un `## 26.`, y la colisión quedó en `develop` sin que nadie la
viera, porque nada miraba. La del allowlist de MCP pasa a **§27** —el número estaba
libre— y las cuatro referencias del BACKLOG, que apuntaban a la del linter, siguen
siendo correctas sin tocarlas.

Arreglar el caso no arregla la clase, así que `scripts/check_doc_links.py` —que ya corre
en CI en cada PR— suma una tercera comprobación: **ningún changelog puede reusar un
número de sección**. Las secciones se citan por número desde el BACKLOG y desde otras
entradas; dos `## 26.` vuelven ambigua cada una de esas citas.

### El guard encontró más de lo que yo buscaba

Seis colisiones, no una: **cinco preexistentes en `CHANGELOG_2026-06.md`**, de una época
con dos corrientes de trabajo numerando en paralelo.

Esas cinco **no se renumeraron**, y la razón está en el código como dato: el BACKLOG cita
"§24 de CHANGELOG_2026-06.md" tres veces para un cambio del router, y **ninguna** de las
dos secciones numeradas 24 en ese archivo es sobre el router. O sea que al menos una cita
ya apunta a otro lado, y elegir cuál entrada se queda con el número lo enterraría en vez
de mostrarlo. Quedan en `KNOWN_SECTION_COLLISIONS`, a la vista, hasta que alguien que
conozca esa historia las resuelva. El objetivo del guard es frenar las **nuevas**, y eso
sí lo hace.

**Verificación.** Dos mutaciones: agregar una sección con un número ya usado → exit 1
nombrando las dos líneas; el mismo duplicado dentro de un bloque de código → exit 0, que
es lo que evita que un ejemplo en la documentación rompa CI.

**Alcance.** Documentación y tooling. Sin cambios de código → ADP no afectado.

---

## 36. El linter entra en CI, en modo reporte

**Qué.** Cierra el último item del track, y el más grande: el linter estaba construido,
probado y documentado, y **no lo corría nadie**. Faltaban dos cosas concretas.

**Modo directorio.** `lint` acepta ahora un directorio y revisa todos los `.json` que
haya debajo. Antes revisaba un archivo por proceso — 303 acá — que es algo que nadie
cablea a CI. Los archivos limpios no imprimen nada en ese modo (trescientos "no findings"
tapan el puñado que importa) y al final va un total.

**Gate por severidad.** `--fail-on error|warning|info|never`, default `never`. `--strict`
queda como alias de `--fail-on warning` para quien ya lo tenga escrito.

Ese default fijo era **la razón concreta** por la que no se podía adoptar: el único gate
disponible fallaba con errores **y** warnings, y este corpus arrastra 75 y 5. Fallaba el
primer día, y un gate que falla el primer día se apaga.

### El paso de CI, y por qué no gatea

```yaml
- name: Graph lint (report only)
  run: cargo run --bin dag_engine -- lint tests/graphs --fail-on never
```

Los hallazgos quedan donde un revisor los ve, sin bloquear a nadie. **Encenderlo es bajar
ese conteo a cero y cambiar una palabra** — `never` por `error`. El flag existe para que
ese día sea eso y no una reescritura.

No se gatea hoy porque arreglar 80 hallazgos en 46 archivos es su propio trabajo, y
mezclarlo acá habría escondido el cambio de herramienta adentro de una limpieza de
corpus.

### Verificación

El gate, corrido en sus cuatro posiciones sobre el corpus real (75 errores, 5 warnings):

| | exit |
|---|---|
| `--fail-on never` | 0 |
| `--fail-on error` | 1 |
| `--fail-on warning` | 1 |
| `--fail-on info` | 1 |
| `--strict` | 1 |

Y sobre un grafo limpio, `--fail-on error` y `--fail-on warning` dan 0. Los 303 grafos en
una sola invocación tardan ~6 s.

Tres tests para `graph_files` (un archivo es él mismo; un directorio da todos los `.json`
a cualquier profundidad y ningún `.md`; un directorio vacío da nada, que el llamador
convierte en error en vez de un "0 archivos, 0 hallazgos" que se lee como aprobado).

**Un test se escribió mal y el código tenía razón.** Afirmaba que los nombres salían
ordenados alfabéticamente; el orden es por **path completo**, así que `agents/deep/c.json`
cae entre `a.json` y `b.json` — que agrupa los grafos de un directorio, que es lo que
quiere quien lee. Se corrigió la aserción, no el sort. Una mutación (`sort` → `reverse`)
lo pone en rojo.

**Alcance.** Aditivo: un flag nuevo con default que preserva el comportamiento, y un
argumento que antes sólo aceptaba archivos. `--strict` significa exactamente lo que
significaba → ADP no afectado.

**Estado.** done.

---

## 37. El motor deja de imprimir el valor que rechaza

**Qué.** Cierra **L4 y L5**, y un tercer sitio que no estaba en la lista y era el peor.

El linter dejó de publicar valores en la §26. Los caminos que toma **el motor** al
rechazar un grafo quedaron abiertos, y los tres imprimían el string ofensor porque serde
renderiza `Unexpected::Str` literalmente:

| Sitio | Lo que imprimía |
|---|---|
| `graph.rs` — `node_schema` malformado (**L4**) | `invalid type: string "sk-live-…"` |
| `validate_mcp_config` — URL no-HTTPS (**L5**) | `got 'http://host/x?token=sk-live-…'` |
| `validate_mcp_config` — bloque `mcp` malformado (**no listado**) | `invalid type: string "Bearer sk-live-…"` |

**El tercero es el más grave** y no estaba anotado: `mcp.headers` es exactamente donde
vive un bearer token, y escribir `headers: "Bearer …"` en vez de un mapa es la forma
ordinaria de equivocarse. Se midió antes de arreglarlo, con un secreto reconocible, para
no afirmar una fuga sin verla.

### Cómo quedaron

```
L4  -> malformed node_schema: `api_key` is a string
L5  -> MCP server URL must be HTTPS, got scheme 'http' (…)
L5b -> the 'mcp' block on this tool is malformed (`headers` is a string, `url` is a
       string). Valid fields are url, transport (…), headers (string map), …
```

Quitar la fuga quitando la información no habría sido un arreglo: los tres siguen
nombrando la clave o el esquema, que es la parte accionable.

**`unreadable_schema_shape` se movió del linter al dominio compartido**, que es lo que el
backlog prescribía: ahora el motor y el linter dicen literalmente lo mismo sobre un
`node_schema` ilegible, por la misma función, y no pueden volver a separarse.

El tercer sitio necesitó una función nueva, `describe_object_shapes`. **Lista todas las
claves, no sólo la ofensora**, y eso es deliberado: serde no dice qué campo le disgustó, y
re-derivarlo significaría repetir la spec acá, donde envejecería. Con cinco claves como
máximo, el lector la encuentra comparando contra la lista de campos válidos que el
llamador ya agrega.

**Verificación.** Tres mutaciones, cada una reintroduce su fuga y pone el test en rojo. Un
cuarto test fija que un bloque `mcp` **válido** sigue aceptándose — una guarda que rechaza
todo no prueba nada — y un quinto imprime los tres mensajes para que se vea que siguen
siendo accionables.

**Alcance.** Cambia el texto de tres mensajes de error. Ningún código de error ni ninguna
condición de rechazo cambió → ADP no afectado, salvo que algo estuviera parseando esos
strings, que nunca fue contrato.

**Estado.** done.

---

## 38. Los números del corpus dejan de medirse a mano

**Qué.** Cierra **L8**. Cada cambio de este track citó `error=75 warning=5 info=0` sobre
los grafos de ejemplo como evidencia de que una regla nueva no agregaba falsos positivos.
**Nada sostenía esos números**: se re-medían a mano cada vez, así que una regla o una
edición del catálogo podía deshacer la reducción de ruido sin que ningún test dijera nada.

`tests/corpus_noise.rs` los fija: total por severidad, **y** el desglose por código —
porque dos cambios que se cancelan dejan el total igual y cambian la mezcla.

### Fijado en las dos direcciones, a propósito

Que el conteo baje **no es automáticamente una buena noticia**. Una regla que deja de
disparar es exactamente cómo se pierde cobertura en silencio, y es la forma que este
archivo existe para atrapar. Las dos mutaciones lo comprueban:

| Mutación | Resultado |
|---|---|
| Una regla **deja** de disparar | 🔴 2 de 3 tests |
| Una regla dispara **de más** | 🔴 2 de 3 tests |

Y un tercer test verifica que el corpus **se está leyendo de verdad** (más de 200
archivos): una medición que silenciosamente no mide nada satisface "cero hallazgos"
perfectamente.

### Que el número se mueva no es un defecto

Agregar un grafo, o arreglar uno, lo mueve legítimamente. Por eso el mensaje de fallo no
finge que el corpus está congelado: dice qué hacer —actualizar las constantes en ese mismo
cambio y decir en el PR qué grafos se movieron y por qué— e imprime el desglose por código
para que la diferencia se lea de un vistazo. Lo que compra la cerca es que el movimiento
tenga que **notarse**, en el cambio que lo causó.

`tests/lint_examples/` queda deliberadamente afuera: esos están rotos a propósito y
ahogarían la señal.

**Alcance.** Sólo tests. Sin cambios de código → ADP no afectado.

**Estado.** done.

**Estado.** done.

---

## 39. El linter espeja las cinco compuertas de una entrada de tool

**Qué.** Cierra **L1**. `MALFORMED_TOOL_ENTRY` cubría una de las cinco compuertas que
`Graph::validate()` aplica a una entrada de tool. Las otras cuatro **rechazan el grafo
entero al cargar y el linter callaba**:

| Compuerta | Dónde vive el chequeo |
|---|---|
| `memory_mode` fuera del enum | inline en `graph.rs` — no hay función de dominio |
| `memory_mode` sobre un tipo de nodo sin memoria | `validate_memory_mode` |
| modo con memoria sin `connection_url` | `memory_backend_missing_reason` |
| bloque `mcp` malformado o URL no-HTTPS | `validate_mcp_config` |

**Tres de las cuatro se llaman, no se copian** — la misma elección que hizo
`node_schema_rejection`. Una reimplementación acá sería libre de divergir de la compuerta
que espeja, y esa divergencia se vería como un grafo que el linter bendice y el motor
rechaza: exactamente el fallo que la regla existe para eliminar. La cuarta no tiene
función de dominio, así que el linter deserializa igual que `graph.rs`.

### Una excepción a la regla de no imprimir valores, y por qué es coherente

El mensaje del enum **sí** imprime lo que encontró, cuando el resto del módulo se niega.
La línea que trazó la §26 no es "nunca imprimir un valor" sino **de dónde viene el
valor**: un enum cerrado que el catálogo declara —como `method`, que
`INVALID_FIELD_VALUE` ya imprime— contra una ranura libre que nombra el autor, como una
clave de `node_schema`. `memory_mode` es lo primero, y el typo es el arreglo. Hacer otra
cosa acá sería incoherente con su propio diagnóstico hermano.

El bloque `mcp` no necesitó excepción: reusa `validate_mcp_config`, que desde la §37 ya
no imprime la URL.

### Verificación

**Cuatro mutaciones, una por compuerta**, todas matan un test. La primera hubo que
reformularla: la versión obvia no compilaba, y una mutación que no compila no prueba
nada — segunda vez en este track.

Un quinto test fija que las entradas que el motor **acepta** siguen sin reportarse: un
`http_request` sin memoria, un `memory_mode: stateless`, y un `mcp` con URL HTTPS.

Corpus `error=75 warning=5 info=0`, sin cambios — y ahora eso lo sostiene la cerca de la
§38, no una medición a mano.

**Alcance.** Aditivo. Ningún grafo del corpus cambia de veredicto → ADP no afectado.

**Estado.** done.

---

## 40. Tres cosas que el linter ya sabía y no decía

**Qué.** Cierra **L1b**, **L3** y **L9**. Con esto el linter espeja **todas** las
compuertas de `Graph::validate()`.

**L1b — el node id con `/`.** El motor lo reserva para calificar paths de subgrafo y
rechaza el grafo al cargar. Es la única compuerta que no es sobre una entrada de tool, y
por eso fue la última: una versión anterior de L1 decía "las otras tres puertas" y la
dejaba fuera de la cuenta. La regla lee el **documento crudo** a propósito: los ids con
prefijo que este módulo construye para hallazgos dentro de un hijo inline (`nested/chat`)
contienen `/` por diseño y no los escribió ningún autor.

**L3 — el consejo del brazo `Registry`.** Un tipo tool-only usado como `type` de un nodo
recibía el did-you-mean genérico bajo `Registry`, mientras que `CatalogOnly` y `Unchecked`
decían dónde va el nombre. Qué `KnownNodeTypes` tenga el llamador cambia **con cuánta
fuerza** el linter puede hablar del motor; no cambia dónde va un nombre.

**L9 — la guarda de campos dentro de `node_schema`.** Resultó ser un hueco de test, no un
defecto: el comportamiento ya era correcto. El test que fijaba que un defecto
independiente sobrevive a una entrada rechazada ponía la clave inventada en
`fixed_config`; una supresión acotada a `node_schema` lo pasaba escondiendo justo la clase
que el test nombra. La mutación ahora la mata.

### El guard de completitud que yo construí era vacuo

Al agregar `INVALID_NODE_ID`, el test que exige un ejemplo por cada `DiagnosticCode`
**siguió en verde**. Su lista de códigos estaba escrita a mano, así que no podía saber de
un código que nadie le contó — exactamente la cobertura vacua que este track viene
persiguiendo, en una herramienta de este track.

El arreglo mueve el forcing function un nivel más abajo: `DiagnosticCode::ALL`, más un
`match` exhaustivo que **no compila** hasta que alguien maneje la variante nueva. El test
ahora deriva de ahí en vez de copiar. Vive en el módulo de tests para que producción no
cargue código muerto —`warnings = "deny"` lo rechazaba— y CI compila los tests, así que
la compuerta dispara igual.

De paso quedaron corregidas dos frases de ese mismo archivo que el cambio volvió falsas:
decía que la lista estaba escrita a mano *en vez de* derivada, y hablaba de dieciocho
archivos cuando son diecinueve.

**Verificación.** Cuatro mutaciones, todas matan un test: no reportar el node id,
reportar todo id, revertir el consejo del brazo `Registry`, y suprimir las reglas de
campos dentro de `node_schema`. Una quinta —quitar la llamada a la regla— **no compilaba**,
tercera vez en el track, y se reformuló.

Corpus `error=75 warning=5 info=0`, sostenido por la cerca de la §38.

**Alcance.** Aditivo: un `DiagnosticCode` nuevo y dos ramas de consejo → ADP no afectado.

**Estado.** done.

---

## 41. Identificadores acotados, y un solo campo nombrado

**Qué.** Cierra **L6** y **L7**, los dos últimos accionables del track.

### L6 — un identificador ya no inunda el reporte

`compact()` acotaba el único lugar que imprime un **valor**. Un `node_type`, un alias de
tool y una clave de config los escribe el autor igual, y nada los acotaba: un `node_type`
de varios KB ahogaba el reporte entero, en texto y en JSON.

No se parchearon las cuarenta interpolaciones a ciegas. Se escribió primero **un test de
propiedad** —ningún mensaje puede escalar con el tamaño del identificador— y ese test fue
señalando, uno por uno, **los cinco sitios que realmente se desbordaban**. Los otros
treinta y cinco interpolan claves del catálogo, acotadas por construcción.

El `field` y el `node_id` de un diagnóstico **no** pasan por ahí: son direcciones —un
consumidor busca el nodo por ellas— y una dirección truncada no apunta a ningún lado.
Sólo se acota la prosa.

### L7 — el motor y el linter nombran el mismo campo

Con dos campos malos en un `node_schema`, el linter nombraba el primero alfabético y
`Graph::validate()` el primero que le daba el `HashMap`. Coincidían en rechazar el grafo y
discrepaban en cuál mostrar, así que un operador arreglaba el que vio en el lint y se
encontraba con el otro al cargar.

`first_parse_rejection` se movió al dominio compartido y ahora **la llaman los dos**. Como
efecto colateral, el mensaje del motor pasa a ser estable entre corridas: antes dependía
del seed del proceso.

**Una advertencia sobre cómo se verificó esto.** El test pasó la primera vez, **antes** de
tocar el motor — y no probaba nada: `RandomState` se siembra una vez por proceso, así que
el `HashMap` da el mismo orden en toda la corrida y repetir el test no varía nada. Lo que
lo prueba es la mutación: revertir el motor a preguntar por el schema entero lo pone en
rojo, nombrando `zulu` donde el linter nombra `alpha`. Un test verde sobre un
comportamiento que depende de un seed es una moneda que salió cara.

**Verificación.** Dos mutaciones: el motor vuelve al orden del `HashMap` → rojo; se
invierte el orden del probe compartido → rojo. Corpus `error=75 warning=5 info=0`.

**Alcance.** Cambia el texto de algunos mensajes y hace determinista uno del motor. Sin
cambios de código de error ni de condiciones → ADP no afectado.

**Estado.** done.

---

## 42. Se limpia el corpus: 74 de 80 hallazgos

**Qué.** La limpieza que hace falta para poder **encender** el gate del linter. De los 80
hallazgos sobre los 303 grafos de ejemplo quedan **6**.

### Tanda 1 — configuración que el motor nunca leyó (46)

| Clave | Veces | Por qué está muerta |
|---|---|---|
| `prefix` en un `log` | 21 | `LogNode::execute` recibe `_config`: ignora su configuración entera |
| `label` en un `output` | 15 | idem `OutputNode::execute` |
| `default_output_port` | 6 | propiedad de nodo que `NodeConfig` no declara |
| `default_input_port` | 4 | idem |

**Se verificó quién tenía razón antes de tocar un grafo.** Veintiuna apariciones de la
misma clave huelen a falso positivo, no a veintiún grafos rotos, así que lo primero fue
leer `debug.rs` y `output.rs`. Los dos toman `_config`. El linter tenía razón.

Veintiún autores escribieron `"prefix": "RESULT:"` esperando que el log lo usara, y **el
motor nunca lo honró**. Que la intención sea razonable es un argumento para una feature,
no para dejar configuración inerte.

### Tanda 2 — los que necesitaban juicio (28)

Cada campo se verificó contra el código antes de borrarlo, y **no todos eran iguales**:

- `reasoning_effort` no es un campo que `llm_call` lea: lo **emite** el adapter de OpenAI
  derivándolo de `thinking_budget`.
- `instructions` pertenece a `output_parser` y `router`, no a `llm_call`.
- `max_steps`, `maxSteps`, `memoryWindow`, `marker_field`: **cero** ocurrencias en todo el
  código Rust.
- Los siete `inputs` a nivel de nodo llevaban plantillas reales que el motor descarta. En
  un `log` y en un `http_request` con edge propio se borran; en dos `llm_call` se mueven a
  `config.prompt`, que **sí** es campo de config y se templa desde los inputs con
  `resolve_template_vars`. Eso es lo que el autor quiso.
- Cuatro `python_script` estaban **doblemente rotos**: campo `script` en vez de `code`, y
  `return` a nivel de módulo, que es `SyntaxError`. Corridos antes y después: antes el
  nodo fallaba con `'code' field is missing`; ahora `data_source` produce los usuarios de
  verdad.

### Dos errores propios, y lo que los atrapó

**El borrado masivo reformateó dos veces.** Un `json.dump` convirtió 46 líneas borradas en
**+1018/−287**, y más tarde 190 líneas para borrar **una** clave. La segunda vez lo agarró
`review_size.py`, no un test: 915 líneas contra un cap de 500. La versión final edita el
texto preservando el formato.

**Un `subgraph` quedó peor que antes.** Se le borró la clave `graph` y quedó sin hijo:
misma falla, menos la intención. El valor era un grafo inline completo, así que lo
correcto era renombrarla a `child_graph_inline`. Restaurada de git y renombrada, el nodo
pasó a funcionar.

**Y esa corrección destapó algo**: con el hijo inline bien nombrado, la recursión de la
§32 entró y encontró un `session_id` inerte que la §20 había removido de otros 30 grafos.
Se había salvado porque hasta esa sección nadie miraba ahí adentro.

### La cerca de la §38 pidió sus números dos veces

Los tests de `corpus_noise.rs` se pusieron en rojo después de cada tanda y exigieron
actualizar las constantes en el mismo cambio, que es para lo que existen.

```
error=75 warning=5  →  error=29 warning=5  →  error=3 warning=3
```

### Los 6 que quedan, y por qué no se tocaron

`advanced/test_orchestrator.json` y `advanced/trip_planner.json` están escritos contra un
contrato **anterior** del orquestador: sus piezas viven como nodos top-level
(`clothing_expert`, `finalizer`, un `planner` aparte) en vez de en el `config`, que es
donde el nodo actual las lee. Arreglarlos es **reescribirlos**, no limpiarlos, y eso pide
decidir qué debe demostrar cada demo — no inventarlo.

**Por eso el gate sigue en modo reporte.** Encenderlo con `--fail-on error` requiere esos
dos, y son el único bloqueante que queda.

**Alcance.** Sólo grafos de ejemplo y las constantes de la cerca. Sin cambios de código →
ADP no afectado.

**Estado.** done.


---

## 43. Se enciende el gate del linter, y se borra el grafo que lo bloqueaba

`tests/graphs/advanced/trip_planner.json` sale del repo y el lint de CI pasa de
`--fail-on never` a `--fail-on error`. El corpus queda en **302 archivos, 0 errores,
3 warnings**.

### Por qué borrar y no arreglar

El grafo reportaba 3 `MISSING_REQUIRED_FIELD` — su `orchestrator` tenía `config: {}`.
Eso es el **cuarto de cuatro defectos independientes**, ya recorridos con el motor real
en agosto y registrados como finding #66 del ledger:

| # | Defecto |
|---|---|
| 1 | `{"from":"trigger","to":"planner"}` sin prefijo `texts.` → el `information_extraction` no recibía nada |
| 2 | `trigger.plan → state_merger.injected_plan` es irresoluble por construcción: un `input` con config no vacía emite exactamente las keys que declara, y `plan` no es una |
| 3 | `state_merger` lee `llm_plan['output']`, pero el payload de un `information_extraction` es el JSON de su `schema` — acá un array, sin key `output` |
| 4 | `orchestrator` con `config` vacío ← **lo único que ve el linter** |

El defecto 1 mataba el run antes de llegar al orquestador, así que el `config` vacío
**nunca se ejercitó**. Los defectos 2 y 3 no son cableado sino diseño del grafo.

Y la forma que hacía distinto a este grafo —un orquestador despachando a nodos
top-level por puertos `dispatched_agents`— **ya no está soportada**
(`orchestrator.rs:1541`):

```
Agent 'X' must be a subgraph: add 'child_graph_path' or 'child_graph_inline'
to its config. Direct LLM agent configs are no longer supported.
```

Reescrito contra el contrato actual sería una copia de dos agentes de
`trip_planner_v2.json`, que ya lintea limpio y cubre el mismo caso con tres. Dos docs
de historia ya lo marcaban `❌ v1, superseded by v2`, y el propio finding #66 cerraba
pidiendo decidir esto antes de repararlo.

### Por qué `error` y no `warning`

Un warning es el linter diciendo que **no puede probar** el hallazgo mirando sólo el
grafo. Un gate que bloquea sobre un quizás admitido se gana el pedido de apagar el gate
entero al primer falso positivo. Lo que bloquea es lo que el linter puede demostrar.

Quedan 3 warnings, todos de `advanced/test_orchestrator.json`, que tiene el mismo
`config` vacío. Se leen como warning y no como error sólo porque su único edge entrante
no nombra puerto, y ahí el linter suaviza asumiendo que el valor podría llegar por el
puerto por defecto. Para el `orchestrator` esa suavización es falsa —
`orchestrator.rs:223` lee `agents` de `config` y no hay un solo `inputs.get("agents")` en
el archivo— así que son **errores disfrazados de warning**. Enseñarle eso al linter es
un cambio propio: BACKLOG **L12**.

### Lo que atrapó cada cosa

La cerca de la §38 se puso en rojo con los números exactos antes de que yo los
escribiera:

```
left:  (302, 0, 3, 0)
right: (303, 3, 3, 0)
```

Y el guard de doc-links marcó las **5** menciones al grafo en docs vivas. Ninguna se
borró: son registros de trabajo pasado —la entrada de agosto que encontró los defectos y
la fila del ledger que los siguió—. El grafo se fue, el hallazgo no. Van al
`GRAPH_REF_ALLOWLIST` con esa razón escrita, que es para lo que existe.

`tests/graphs/AUDIT_RESULTS.md` listaba este grafo como `✅ OK`. No lo era: esa auditoría
verificó que los grafos **cargan**, no que llegan a un resultado. Queda dicho ahí.

### E2E

Lo que se borró es un grafo que no llegaba al final en ninguna de sus formas. Lo que lo
reemplaza sí. `trip_planner_v2.json` corrido por el motor contra Gemini real
(`gemini-2.5-flash`), con el criterio que pedía el propio finding #66 — *"assert it
reaches finish with a populated final_response and zero node-skipped frames"*:

```
finishReason: "stop"        node-skipped: 0
agentes que corrieron: budget_expert, clothing_expert, gear_expert
totalTokens: 5828 (prompt 3638, completion 806, thinking 1384)
```

La respuesta final llega poblada: lista de ropa, lista de equipo y un total de $1555.
Los tres agentes son `child_graph_inline`, que es la forma que el contrato actual exige y
la que el grafo borrado no usaba.

- Grafo: `tests/graphs/advanced/trip_planner_v2.json`
- CI: `.github/workflows/ci-develop.yml`
- Cerca: `src/libs/colmena/tests/corpus_noise.rs`
- Ledger: finding #66, cerrado
## 44. `mcp.dispatch_failed` distingue clases de fallo, y `mcp.dispatch_refused_secret` ya lleva `tool_call_id`

**Qué cambió.** Cierra los dos huecos que la entrada #25 había dejado explícitos
como pendientes.

**Antes.** Un timeout, un fallo de transporte, un error reportado por el servidor,
una ruta no expuesta y un rechazo por secure value producían la MISMA línea WARN
(`tool`, `tool_call_id`, `ms`): el `McpError` se absorbía en el texto contenido que
lee el modelo y nunca llegaba al log. Un operador triando un tercero degradado no
podía distinguir "lento/colgado" de "alcanzable pero fallando". Además,
`mcp.dispatch_refused_secret` no llevaba `tool_call_id` mientras su hermano sí —
como las tool calls de un turno corren concurrentes (`JoinSet` en `llm.rs`), unir
dos rechazos simultáneos a la misma tool por `tool` + tiempo podía fallar.

> Nota posterior (2026-09-25): lo del `JoinSet` era falso. Ese `JoinSet` de `llm.rs` resume adjuntos; las tool calls de un turno corrían en serie. Ver la [entrada 97](#97-un-grupo-de-llamadas-parallel-corre-a-la-vez-parallel-tools-2e).

**Ahora.** `McpDispatched` lleva un campo `kind: DispatchKind` (nuevo tipo en
`dispatch.rs`, mismo patrón que `FetchFailure` en `wire.rs`) con una etiqueta
snake_case estable por cada camino real: `ok`, `unrouted`, `unbound`,
`refused_secret`, `server_error`, y una por cada variante de `McpError`
(`timeout`, `transport`, `handshake`, `protocol`, `tool_not_found`,
`tool_call_failed`, `schema_too_large`, `invalid_config`). `mcp.dispatch_failed`
ahora incluye `kind = dispatched.kind.label()`; `mcp.dispatch_ok` no lo lleva —
su `kind` es siempre `ok`, así que sería un campo constante y puro ruido.
`tool_call_id` ahora viaja hasta `call_and_contain` y llega a
`mcp.dispatch_refused_secret`, así que las dos líneas que un rechazo por secreto
produce (`mcp.dispatch_refused_secret` y `mcp.dispatch_failed` con
`kind = "refused_secret"`) se unen por id, no por heurística.

**Dónde vive el mapeo.** `impl From<&McpError> for DispatchKind` en `dispatch.rs`
(infraestructura), no en `llm::domain::mcp` — ese módulo tiene la regla dura de
cero dependencias de infraestructura, y `DispatchKind` nombra conceptos que el
dominio no conoce (`unrouted`/`unbound` son de la tabla de rutas del dispatcher).

**Alcance.** Instrumentación pura: el texto contenido que ve el modelo, el flag
`failed`, el `ToolResult` devuelto, el control de flujo y el orden
rechazo-antes-que-red no cambian. Sin cambio de API pública; ADP no afectado.
Referencia completa, incluida la tabla de `kind` y su lectura operacional, en
[`developer_guide/52`](developer_guide/52_mcp_observability.md).

**Estado.** done.

---

## 45. El linter dice cómo se ve el campo bien escrito

`MISSING_REQUIRED_FIELD` decía qué faltaba y nunca qué forma tenía que tener. Ahora
cita el ejemplo que el catálogo ya traía escrito.

```
error [MISSING_REQUIRED_FIELD] node "orch".agents: required field "agents" is not set,
and no incoming edge supplies it — the catalog documents it as
{"flights_agent":{"description":"Searches for flights using the Amadeus API",
"child_graph_inline":{"nodes":{"in":{"type":"input","config":{}},...}}}}
```

No es información nueva. `docs/node_configurations.json` declara un `example` en **156 de
sus 237 campos**, el linter **ya cargaba ese archivo**, y el campo `suggestion` del
diagnóstico ya existía y se usaba para otros códigos. Lo único que faltaba era conectarlo.

De los 40 campos requeridos con ejemplo, la mediana mide 19 caracteres y el mayor 280 —
`orchestrator.agents`, que es justamente el que muestra la forma que le faltaba al grafo
borrado en la §43.

### Dos reglas, y por qué

**No se inventa un ejemplo cuando el catálogo no tiene uno.** Quien lee no puede
distinguir uno inventado de uno documentado, así que una suposición errónea cuesta más
que el silencio.

**No se emite JSON truncado.** Por encima de `MAX_INLINE_EXAMPLE` (400) el mensaje apunta
al catálogo en vez de imprimir un prefijo del objeto: un objeto cortado se lee como
copiable y no lo es. Hoy ningún ejemplo llega al límite, y un test afirma eso — cruzarlo
será una decisión, no una sorpresa.

El warning suavizado **conserva su matiz y suma la cita**: `…may arrive through its
default input port instead; if it does not, the catalog documents it as …`. Es donde más
sirve, porque es donde quien escribe está adivinando la forma.

### Dónde vive el ejemplo, y por qué no en `FieldSpec`

Fuera de `NodeCatalogEntry`, en un `field_examples` propio — el mismo lugar y la misma
razón que `input_ports`. La fase 2 del linter compara el `config_schema()` de cada nodo
contra la entry **entera**, y el alcance acordado de esa declaración son los hechos
mecánicos. Meter prosa adentro obligaría a los 37 nodos a declarar documentación que no
les corresponde, y convertiría cada mejora del catálogo en un test roto.

### Lo que se verificó, no lo que se supuso

Las dos mutaciones se corrieron:

| Mutación | Resultado |
|---|---|
| La cita se calcula pero no llega al diagnóstico | **2 tests caen** |
| El límite se ignora, todo va inline | **1 test cae** |

La primera hubo que reformularla: escrita como "que `example_clause` devuelva `None`"
**no compila** — `warnings = "deny"` convierte la función huérfana en error de build, y
una mutación que no compila no prueba nada.

Y el primer test del límite era **vacuo**: construía el valor grande y nunca se lo pasaba
a la función, así que sólo ejercitaba la rama inline. Se partió `phrase_example` de la
búsqueda en el catálogo para que la decisión de tamaño sea alcanzable con un valor de
cualquier medida; probarla sólo a través del catálogo fija la rama que los ejemplos de hoy
toman y deja la otra sin ejercitar hasta que una edición del catálogo la alcance en
producción.

### Lo que encontró el E2E: el consejo no se podía seguir hasta el final

Se armó un grafo usando **sólo** los tres ejemplos que el linter cita, sin escribir nada
a mano. Pasó de 3 errores a `no findings` — el consejo es aplicable literalmente. Pero al
correrlo por el motor:

```
error: Missing 'provider' in inputs or config
```

El ejemplo de `orchestrator.agents` traía un `llm_call` interno con `config: {}`. Como
documentación de la **forma** está bien; copiado tal cual, lintea limpio y muere al
ejecutar. Un consejo que no llega hasta el final es medio consejo, y ése es justamente el
punto de esta sección.

No hizo falta inventar un criterio: **el ejemplo de `subgraph`, en el mismo archivo, ya lo
hacía bien** — trae `provider`, `model` y `api_key`. El de `orchestrator` era el que se
había quedado afuera. Se lo alineó además con sus propios hermanos `planner` y
`final_reactor`, que usan google/gemini-2.5-flash.

**Una línea de cambio**, con una guarda estructural que verifica que el JSON parseado
difiera *sólo* en ese config — la misma disciplina que en la §42, donde un `json.dump`
descuidado convirtió 46 líneas borradas en +1018/−287.

Con eso, el ciclo cierra entero:

```
lint:   no findings
run → suspend → resume:   finishReason: "stop"
node-skipped: 0   ·   errores: 0   ·   totalTokens: 2517
```

El `planner` del ejemplo trae `allow_suspend: true`, así que el primer run **suspende** y
pregunta ciudad, fechas y viajeros. Eso es comportamiento correcto, no defecto — pero
significa que sin responder las preguntas los agentes nunca corren, y una verificación que
se hubiera detenido en el primer `exit=0` no habría visto el `config: {}` roto.

### Alcance

`MISSING_REQUIRED_FIELD` solamente. Los otros códigos que podrían citar un ejemplo
(`FIELD_TYPE_MISMATCH`, `INVALID_FIELD_VALUE`) quedan fuera a propósito: ya dicen lo que
aceptan, y ampliar el alcance acá sólo agranda el cambio.

Cierra BACKLOG **L13**. Sin cambio de API pública → ADP no afectado.

---

## 46. El corpus llega a cero, y el último grafo roto se reescribe

`advanced/test_orchestrator.json` era el único hallazgo que quedaba: 3 warnings
`MISSING_REQUIRED_FIELD` por un `orchestrator` con `config: {}`.

```
303 archivos: 0 error(s), 3 warning(s)   →   303 archivos: 0 error(s), 0 warning(s)
```

Con eso el track cierra su medición: **de 80 hallazgos a cero**.

### Por qué se reescribió y no se borró

Es el mismo contrato muerto que el `trip_planner.json` de la §43 —despacho a nodos
top-level por puertos `dispatched_agents`— pero acá había un motivo no-duplicado para
conservarlo: **no existía en el corpus un orquestador mínimo escrito contra el contrato
actual**. Los 13 orquestadores de un agente que hay ejercitan features específicas
(critic feedback, HITL, anidamiento); ninguno es el caso base, y la guía de ejemplos
señala a éste como "orquestador básico".

De paso arregla algo que el linter **no** puede ver: sus dos agentes eran nodos `log`.
CLAUDE.md lo prohíbe explícitamente — un placeholder como backing convierte la prueba en
un mock y esconde fallos reales de ejecución. Ahora es un `packing_expert` de verdad.

La forma salió del ejemplo del catálogo, el mismo que la §45 dejó verificado y ejecutable.

### Lo que corrigió esto en el BACKLOG

Preparando **L12** se midió el motor y **el encuadre del ítem era incorrecto**. Decía que
el valor "no puede llegar por el puerto de entrada por defecto". Sí puede:
`build_inputs_for` usa `default_input()` del nodo para un edge sin punto, y si el nodo no
declara ninguno —**sólo 12 de los 37 lo hacen**, y el `orchestrator` no está entre
ellos— cae en auto-flatten y mergea todas las claves del objeto upstream.

Lo que ocurre es otra cosa: el valor llega y **se ignora**. El `orchestrator` sólo lee
`plan`, `prompt` y `user_message` desde `inputs`. La pregunta correcta no es "¿puede
llegar?" sino "¿el nodo lo lee de ahí?" — y eso cambia el arreglo, que ahora está escrito
como corresponde en L12.

### E2E

```
tests/graphs/advanced/test_orchestrator.json · gemini-2.5-flash
finishReason: "stop"   node-skipped: 0   errores: 0
agentes que corrieron: packing_expert      totalTokens: 1793
```

Respuesta poblada, con lista de equipo y ropa.

---

## 47. Sale del repo una credencial de base de datos

Tres archivos versionados llevaban una cadena de conexión completa —usuario, contraseña,
host y base— a la instancia Postgres compartida.

| Archivo | Qué era | Arreglo |
|---|---|---|
| `src/libs/colmena/output.log` | un log de ejecución commiteado por accidente | fuera del versionado + `.gitignore` |
| `tests/graphs/external/adp_canvas_load_test.json` | la cadena literal en `connection_url` | `${DATABASE_URL}`, el placeholder que el catálogo documenta |
| `docs/superpowers/plans/2026-04-29-adp-deploy-with-skills.md` | dos citas del script de deploy | valor redactado, forma conservada |

### El origen ya estaba cerrado; esto son las copias

La credencial no se escribió a mano en el log: **el motor la imprimía en cada arranque**
(`DEBUG: DATABASE_URL=Ok("postgresql://…")`), y el log commiteado fue el lugar donde quedó
una copia visible. Ese `print` se removió en `d273a666`; hoy `engine.rs:104` lee la
variable con `std::env::var` y sólo la nombra en el mensaje de error **cuando falta**,
nunca su valor. Verificado sobre el código en disco, no sobre el recuerdo.

Vale nombrar el alcance que eso implica: mientras el `print` existió, la credencial pudo
quedar en cualquier log de cualquier ejecución, no sólo en el que terminó en git.

### Lo que este cambio NO hace

**No cierra la exposición.** El valor sigue en el historial de git, y también en el repo
de ADP, donde `deploy_gcp.sh` lo tiene como *default operativo* — si alguien corre ese
script sin `DATABASE_URL` en el entorno, despliega los servicios con esa credencial. Eso
va en un cambio aparte, en ese repo.

**Lo único que cierra el acceso es rotar la contraseña en Cloud SQL.** Borrar archivos no
la saca del historial, y mientras siga siendo válida el historial alcanza. La rotación
corta `colmena-worker`, `colmena-api` y el job `attachment-gc` hasta que se redespliegan,
así que necesita ventana y coordinación — no es una acción de este PR.

### La regla, que importa más que el borrado

`**/output.log` entra al `.gitignore` con el motivo escrito al lado. Sacar el archivo
arregla este caso; la regla evita el siguiente, que es donde estaba el agujero real: un
log de ejecución versionado publica lo que sea que el proceso haya impreso ese día.

## 48. Los schemas de un servidor MCP se conforman al dialecto que Gemini acepta

**Qué cambió.** Un `llm_call` con `provider: google` y un servidor MCP cuyo schema use
cualquiera de 14 keywords de JSON Schema ya no falla. Antes fallaba **entero**: no la tool
que traía la keyword, ni las tools de ese servidor — la request completa, con todos los
built-ins del agente.

### El caso que lo destapó

El MCP remoto de GitHub (`https://api.githubcopilot.com/mcp/`) publica la extensión
`x-mcp-header` **dentro de cada `properties.<campo>`**. Con 16 tools expuestos, Gemini
devolvió 52 errores idénticos y cero tool calls:

```
Invalid JSON payload received. Unknown name "x-mcp-header"
  at 'tools[0].function_declarations[N].parameters.properties[M].value'
```

El mismo grafo, sin tocar nada, funcionó contra Anthropic y OpenAI: ambos llamaron
`github__list_pull_requests` y devolvieron datos reales. El defecto era de un solo provider.

### Por qué no alcanzaba lo que ya había

`parameters` en Gemini no es JSON Schema: es un protobuf, y un protobuf rechaza todo nombre
que no declara. Medido contra la API viva (`gemini-2.5-flash`), **14 de 32 keywords comunes
son rechazadas**: `$schema`, `$id`, `$ref`, `$defs`, `definitions`, `additionalProperties`,
`examples` (el plural — `example` singular sí pasa), `const`, `exclusiveMinimum`,
`exclusiveMaximum`, `multipleOf`, `uniqueItems`, `deprecated`, `readOnly`, `writeOnly`, y
toda extensión `x-`.

Colmena tenía **dos saneadores de fuerza desigual, y el débil cuidaba lo ajeno**:

| Saneador | Qué quitaba | A qué se aplicaba |
|---|---|---|
| `llm_synthetic_tools/mod.rs` | recursivo; `$schema`, `additionalProperties`, inlinea `$ref` | schemas del repo |
| `mcp/expose.rs` | `$schema` y `$id`, **sólo en el nivel superior** | schemas de terceros |

Entre los dos cubrían 3 de las 15 claves. Y ambos eran **denylists sobre una gramática que
es un allowlist**, así que sólo podían enumerar los fallos ya vistos. El comentario del
propio `expose.rs` lo admitía: *"what is fixed is what was observed to break"*.

### La solución

Un módulo nuevo, `llm/infrastructure/gemini_schema.rs`, con un **allowlist** derivado de
sondear la API viva keyword por keyword. Se aplica en el adapter de Gemini, sobre
`input_schema_override` — la única puerta por la que entra JSON que Colmena no escribió (el
camino normal pasa por `ParameterProperty`, un struct tipado que no puede cargar claves
arbitrarias).

**En el adapter, no en el módulo MCP**, y eso es deliberado en dos sentidos. Uno de
evidencia: Anthropic y OpenAI aceptan las 15 claves, así que filtrar del lado de MCP les
borraría restricciones que sí honran (`const`, `examples`, `exclusiveMinimum` son
validaciones reales, no metadata). Otro de arquitectura: saber qué dialecto habla Google no
es asunto del módulo que habla MCP.

Dos decisiones que vale la pena no re-discutir:

- **Allowlist, no denylist.** Lo habilita una asimetría medida: un schema de propiedad
  vacío `{}`, o sin `type`, la API lo acepta (200). Entonces borrar de más cuesta una
  restricción; borrar de menos cuesta el turno entero. Los modos de fallo no son
  comparables.
- **Se descarta, no se traduce.** `const: "x"` no se convierte en `enum: ["x"]` por
  tentador que sea. Traducir es decidir que nuestra lectura del schema de un tercero le gana
  a lo que escribió, y un error ahí cambia en silencio qué argumentos cree válidos el
  modelo.

### Hueco conocido: `$ref`

`$ref` se descarta como cualquier otra clave no aceptada, pero ese descarte es el único que
**no** es benigno: se lleva la definición entera de la propiedad y deja `{}`, sin error en
ningún lado. Queda anotado y con un test que lo fija, porque sigue siendo estrictamente
mejor que hoy — donde ese mismo schema tumba la request completa. Inlinear refs desde
`$defs`/`definitions` tiene sus propios modos de fallo (ciclos, URLs externas, definiciones
ausentes) y va en su propio cambio.

### Verificación

9 tests unitarios, **mutados para probar que son portantes**: sacar la recursión en
`properties` mata cinco, sacarla en `items`/`not` mata dos, y desactivar el allowlist mata
siete.

E2E vivo contra el MCP real de GitHub con credencial real
([`tests/graphs/agents/mcp_github_credentialed_e2e.json`](../tests/graphs/agents/mcp_github_credentialed_e2e.json)),
en los tres providers:

| Provider | Antes | Después |
|---|---|---|
| Gemini | 52 × 400, 0 tool calls | 3 tool calls, 0 errores |
| Anthropic | 3 tool calls | 3 tool calls (sin cambio) |
| OpenAI | 3 tool calls | 3 tool calls (sin cambio) |

La respuesta del modelo se corroboró contra `gh pr list` — coincide, no es alucinación.

El grafo commiteado usa `"Authorization": "<sv_github_token>"`, que es la forma de
producción; la corrida viva se hizo con un token literal inyectado localmente y **no
commiteado**.

### Alcance

Sólo Gemini, y sólo `input_schema_override`. Dos seguimientos, cada uno con su propio E2E:
inlinear `$ref`, y retirar el saneador viejo de `mcp/expose.rs` — que cambia lo que reciben
Anthropic y OpenAI. Sin cambio de API pública → ADP no afectado.

## 49. Un `$ref` de un servidor MCP se inlinea en vez de borrar la propiedad entera

**Qué cambió.** Cierra el hueco que la §48 dejó anotado a propósito. Un schema MCP que use
`$ref` con `$defs` o `definitions` ya no pierde la definición de esa propiedad cuando el
provider es Gemini: el cuerpo referenciado se copia al sitio de la referencia antes de
filtrar.

### Por qué este descarte era distinto a los otros

La §48 estableció que descartar una clave no aceptada es benigno: el modelo pierde una
restricción y el servidor sigue validando su propia entrada. `$ref` es la excepción. No
lleva una restricción — **es** la definición. Descartarlo deja `{}`, o sea un parámetro del
que el modelo no sabe absolutamente nada, y sin error en ningún lado que lo diga.

Medido contra la API viva, las tres formas del mismo schema:

| Forma | Resultado |
|---|---|
| Cruda, como la publica el servidor (`$ref` + `$defs`) | **400** — tumba la request entera |
| Lo que producía la §48 (`{"order": {}}`) | 200, pero el modelo elige a ciegas |
| Lo que produce este cambio (inlineado) | 200, y el modelo devolvió `{"order": "asc"}` |

Ese último renglón es el punto: con el enum presente el modelo eligió un valor válido del
enum. El inlining no es cosmético.

### Cómo

`inline_refs` levanta `$defs` y `definitions` de la raíz y sustituye cada `#/$defs/X` por el
cuerpo de `X`. Sólo lee la raíz: JSON Schema permite definiciones en cualquier nivel, pero un
puntero se escribe contra la raíz del documento, así que un mapa anidado no es direccionable
por los refs que esto resuelve.

Tres detalles que valen su comentario en el código:

- **Transitivo.** Un cuerpo que a su vez apunta a otra definición se resuelve también.
  Resolver sólo el primer salto dejaría un `$ref` atrás para que el filtro lo descarte — o
  sea el mismo fallo, un nivel más abajo.
- **La sustitución camina con la profundidad AUMENTADA**, no con una fresca. Eso es lo que
  hace que una definición auto-referencial (`Node.child: $ref Node`) termine en la cadena de
  referencias en vez de consumir el presupuesto de anidamiento del schema.
- **Un ref irresoluble se descarta**, no se reenvía. Una URL externa o una definición que el
  servidor no mandó degradan a `{}`, que es lo que honestamente significa una referencia que
  no se puede seguir.

### Verificación

13 tests unitarios (5 nuevos), **mutados**: no llamar a `inline_refs` mata 4; resolver sólo
el primer salto mata 1; ignorar la forma draft-04 `definitions` mata 1; y pasar profundidad
fresca en el salto revienta el proceso en el test del ciclo, que es exactamente lo que el
bound evita.

**Sobre el E2E: no hay servidor MCP alcanzable que publique `$ref`.** Lo medí en los tres —
DeepWiki (3 tools), Context7 (2 tools) y GitHub con todos sus toolsets (44 tools): cero
`$ref`, cero `$defs`, cero `definitions`. Así que la evidencia de comportamiento nuevo es la
tabla de tres formas de arriba, contra la API real de Gemini, y no una corrida de grafo.

Lo que **sí** se corrió E2E es la regresión: el mismo grafo de la §48 contra el MCP real de
GitHub sigue en 3 tool calls y 0 errores de schema.

### Alcance

Sólo Gemini. Queda un seguimiento: retirar `without_schema_metadata` de `mcp/expose.rs`, que
quedó subsumido — cambia lo que reciben Anthropic y OpenAI, así que necesita su propio E2E.
Sin cambio de API pública → ADP no afectado.

## 50. Corrección: el saneador de `mcp/expose.rs` NO se retira, y el comentario que decía por qué existe estaba viejo

**Qué cambió.** Sólo comentarios. Ningún cambio de comportamiento.

Las §48 y §49 anunciaron un seguimiento: retirar `without_schema_metadata` de
`mcp/expose.rs` porque quedaba "subsumido" por el conformador nuevo. **Ese anuncio estaba
mal, y esta sección lo corrige** — `develop` es compartido y una afirmación equivocada ahí
no se puede reescribir, sólo corregir.

### Por qué no se retira

Al ir a borrarlo apareció que sólo quedaba subsumido **para Gemini**. Para los otros dos
providers sigue haciendo un trabajo real, y son dos trabajos distintos, no una duplicación:

| Función | Trabajo | Alcance |
|---|---|---|
| `without_schema_metadata` | quita metadata del documento (`$schema`, `$id`) | agnóstico del provider |
| `gemini_schema::conform` | conforma al protobuf de Gemini | específico del provider |

`$schema` y `$id` no los lee ningún modelo. Mandarlos gasta tokens en **todos** los
providers a cambio de nada: medido sobre el catálogo vivo de Context7, unos **59 bytes por
tool, ~6% de sus bytes de schema**, que Anthropic y OpenAI aceptan encantados y tiran.

Y hay un segundo efecto que el borrado habría roto en silencio: el techo de 32 KB **mide el
schema saneado**. Sacando el saneo, mediría bytes que ningún provider recibe.

### La lección, que es la parte que importa

**El defecto original no era "hay dos saneadores".** Era *un* saneador haciendo el trabajo de
un provider, mal: un denylist de dos claves, sólo en el nivel superior, custodiando un
protobuf que rechaza 14 de 32 keywords. Mover el trabajo de dialecto al adapter arregló eso.
Borrar lo que quedó no simplificaba nada — mandaba bytes inútiles a dos providers y dejaba el
techo midiendo algo que nadie recibe.

"Quedó subsumido" era una inferencia razonable desde la forma del código y falsa contra el
código.

### Qué se arregló entonces

El comentario, que había quedado activamente engañoso. Decía:

> *"Provider schema dialects differ in more ways than this and a general translation layer is
> a real design problem, not something to improvise here."*

Esa capa **ya existe** desde la §48. Alguien leyendo eso hoy concluiría que no está hecha.
Ahora el comentario apunta a `gemini_schema::conform`, dice por qué esta función igual se
queda, y el del techo aclara que su medida es **exacta para Anthropic y OpenAI y una cota
superior para Gemini**, cuyo adapter conforma todavía más.

Sin cambio de comportamiento → sin E2E nuevo. La suite entera pasa.

## 51. Fix: el suffix temporal volátil nunca cacheaba en la OpenAI Responses API

**Qué cambió (parte 1 — 2026-09-09).** `build_responses_request_body`
(`llm/infrastructure/openai_adapter.rs`) concatenaba el bloque temporal/geográfico
volátil al final del **último** mensaje `system` de `input` — la misma regla que
funciona en Chat Completions, Anthropic y Gemini. En `/v1/responses` esa colocación
**nunca cacheaba**: medido en vivo, write N/read 0 en cada turno, tanto concatenado al
system existente como como system message separado. El fix empuja el bloque como un
**item `system` nuevo, incondicionalmente el último elemento de `input`**, sin
modificar ningún item existente — y en particular sin intentar adjuntarlo a un
`function_call_output` (ese item no tiene `content`). Con esa colocación: turno 1
write ~2426/read 0 (cold, esperado), turnos 2+ write ~31/read ~2395.

**Qué cambió (parte 2 — 2026-09-10).** La parte 1 era **necesaria pero insuficiente**.
Bisección en vivo sobre el body real capturado ([user, system estable, system
volátil], que es el orden que Colmena emite — `[User, System, ...]`) cambiando SÓLO
la posición del item `system` estable mostró que la misma llamada, con ese body
verbatim, escribía 2733/leía 0 en la llamada 2 — pero moviendo únicamente el system
estable al frente (sin tocar la colocación del bloque volátil, que sigue último) esa
MISMA llamada pasó a escribir 0/leer 2733. `build_responses_request_body` ahora
reordena `input` en el wire — nunca la historia persistida — en tres pasadas: todos
los items `System` salvo el bloque volátil, preservando su orden relativo; luego el
resto de los items (user/assistant/function_call/function_call_output), también en su
orden relativo original; y por último el bloque volátil, como antes. Una conversación
compactada, que lleva DOS mensajes `System` (secciones estables + resumen de
compactación), mueve ambos al frente en su orden original — el resumen de
compactación queda en la posición 1, no en la 0, así que el prefijo cacheable llega
hasta el resumen.

### Por qué "al final del mensaje" no era lo mismo que "al final de `input`"

La intención original (spec `2026-06-11-temporal-block-cache-safe-design.md` §2.1) era
"el bloque volátil se inyecta fuera del prefijo cacheado". En Chat Completions,
Anthropic y Gemini, "al final del último system message" ya cumple eso — el prefijo
cacheable termina ahí. En `/v1/responses` el motor de caché mide el `input` completo
como secuencia de items, no el texto de un item aislado: un bloque volátil que termina
un item que NO es el último de `input` sigue teniendo bytes estables detrás (el resto
de `input`), así que el prefijo nunca es byte-idéntico entre turnos y nunca cachea. La
regla operativa, falsificable, verificada por las tres colocaciones medidas: **nada
estable puede seguir a los bytes volátiles**. No se afirma un mecanismo interno de OpenAI
más allá de lo medido.

Efecto colateral limpiado: el fallback de "no hay system message → insertar uno al
frente" (`last_system_idx`/`suffix_applied`) quedó subsumido por la regla nueva —
empujar incondicionalmente al final cubre ambos casos — y se eliminó.

### Por qué la posición 0 también importa

Colmena emite la historia como `[User, System, ...]` (ver `SUMMARY_KEEP_FIRST_MSGS`,
`history_compaction` y el coalescer en `LlmRequest::new`, que nunca se tocan aquí — ver
"Alcance"). Sin la reordenación de la parte 2, el primer item de `input` en
`/v1/responses` es el prompt del usuario, no el contenido `System` estable. La
bisección del 2026-09-10 mostró que eso basta para que el turno 2 no lea de caché,
incluso ya con el bloque volátil correctamente al final: la segunda condición,
**el contenido `System` estable debe empezar en `input[0]`**, es tan necesaria como la
primera. Ambas están medidas sobre el mismo body capturado, cambiando una sola
variable por vez — no se afirma ningún mecanismo interno adicional de OpenAI.

### Alcance

Sólo `build_responses_request_body` (ruta gpt-5-family + tools), y sólo la
serialización — el ORDEN en que los items se escriben al wire. `build_messages`
(Chat Completions), el adapter de Anthropic y el de Gemini quedan sin cambios —
confirmado con tests de regresión y con medición viva en gpt-4o (`cache_read` ≈2176,
sin cambios). La historia persistida (`llm_call`, `history_compaction`,
`SUMMARY_KEEP_FIRST_MSGS`, el coalescer de `LlmRequest::new`) tampoco cambia — sus
índices siguen siendo válidos, sólo cambia cómo esta función particular arma el
array `input` a partir de ellos. Sin cambio de API pública, sin cambio de wire-format
hacia ADP.

### Documentación de referencia

- Spec: [`docs/superpowers/specs/2026-06-11-temporal-block-cache-safe-design.md`](superpowers/specs/2026-06-11-temporal-block-cache-safe-design.md)
  (addendum 2026-09-09, extendido 2026-09-10).
- Dev guide: [`docs/developer_guide/35_temporal_geographic_context.md`](developer_guide/35_temporal_geographic_context.md),
  [`docs/developer_guide/14_llm_deep_dive.md`](developer_guide/14_llm_deep_dive.md) §14.

### Estado

Done (unit tests + mutation check, ambas partes). El gate E2E vivo contra
`/v1/responses` con la forma `function_call_output`-tail queda fuera de esta entrega
(se maneja por separado).

## 52. Refactor: el resolver de templates de `llm_call` pasa a `nodes/util/template.rs`

**Qué cambió.** El cuerpo de `LlmNode::resolve_template_vars` (resolución de
`{{key}}` y `{{key.nested.path}}` contra `inputs`) se movió sin cambios de
lógica a una función compartida, `nodes::util::template::render_template`. La
única diferencia de firma es que quien llama decide dónde se busca la clave
raíz: `llm_call` sigue buscando en sus `inputs` a través de un wrapper de una
línea, y sus tres call sites no se tocaron.

**Por qué.** Es la primera mitad del arreglo del hallazgo A4 de
[`qa/nodes/RESUMEN_GAPS.md`](qa/nodes/RESUMEN_GAPS.md): el nodo `input` resuelve
`{{key.nested}}` con un lookup plano y sin traversal, y la segunda mitad lo
conectará a este mismo resolver en vez de mantener una segunda copia de la
misma semántica. Este PR no cambia todavía el nodo `input`.

**Qué cambia en runtime.** Nada en el texto renderizado. Lo único nuevo es un
evento `tracing::debug!` (target `colmena::dag_engine::template`, campo `path`,
nunca el valor) cuando una clave o un path no existe; antes el fallo era
completamente silencioso.

**Tests.**

- 5 tests de caracterización en `llm.rs` (clave plana, dot-path, valor no
  string como texto JSON, clave inexistente → `""`, `{{` sin cerrar queda
  literal). Se escribieron y pasaron contra el cuerpo original, y siguen verdes
  sin editar después del movimiento.
- 5 tests unitarios directos sobre `render_template`.

**E2E.** Grafo de un solo uso (no versionado; la segunda mitad agrega el E2E
permanente del nodo `input`), corrido con `dag_engine run` contra Gemini 2.5
Flash: un nodo `input` entrega `{"plano": "VALOR_PLANO_7431", "usuario":
{"nombre": "Ana_9912"}}` al puerto nombrado `eco.datos` de un `llm_call` cuyo
`system_message` es `A={{datos.plano}} B={{datos.usuario.nombre}}
C=[{{no_existe}}] D={{datos.usuario}}`, y otro nodo `input` le entrega el
`prompt`. El modelo respondió
`A=VALOR_PLANO_7431 B=Ana_9912 C=[] D={"nombre":"Ana_9912"}`.

Un detalle que costó una corrida: `llm_call` declara `default_input = "prompt"`,
así que un edge **sin puerto** hacia él entrega solo la clave `prompt` del
origen y nada más. Con los datos por ese camino, los mismos templates
renderizaron vacíos (`A= B= C=[] D=`) — no por el resolver, sino porque las
claves nunca llegaron a `inputs`. Para que un template de `llm_call` vea datos
de otro nodo, el edge tiene que nombrar el puerto.

### Alcance

Solo infraestructura: `nodes/util/template.rs` (nuevo), `nodes/util/mod.rs`,
`nodes/llm.rs`. Sin cambio de API pública ni de wire-format; ADP no se ve
afectado.

## 53. Fix: el nodo `input` resuelve `{{key.nested}}` y los valores que llegan por edge

**Defecto (A4 de [`qa/nodes/RESUMEN_GAPS.md`](qa/nodes/RESUMEN_GAPS.md)).**
`input.rs` resolvía `{{...}}` con `state.get(key)` literal: sin traversal y solo
contra `state`, que no contiene outputs de otros nodos. Casi todo template
renderizaba `""` en silencio; solo resolvían claves de `state` como
`{{session_id}}`.

**Fix.** El nodo usa el resolver compartido de §52 buscando la clave raíz
**primero en `state` y después en `inputs`**. Dot paths e índices de array se
recorren, un no-string se renderiza como texto JSON y una clave ausente sigue
dando `""`. `state` va primero (al revés que `llm_call`) porque una clave puede
traer valores distintos en ambas fuentes (`session_id`, `plan`,
`__colmena_subgraph_depth` entregados por edge): así toda clave que ya resolvía
produce los mismos bytes. Como tool o dentro de `for_each`, `state` está vacío.

**Puertos nombrados.** Un edge con puerto (`"to": "plantilla.origen"`) entrega el
payload solo bajo esa clave (`{{origen.plano}}`); uno sin puerto aplana las
claves del origen (`{{plano}}`) y ahí `{{origen.plano}}` da `""`. Dos edges sin
puerto con la misma clave colapsan en una sin aviso — comportamiento
preexistente del motor, no tocado aquí; el remedio es nombrar el puerto (ver
[`16_data_flow_guide.md`](developer_guide/16_data_flow_guide.md)).

**Tests.** Módulo nuevo en `input.rs`. Contra el lookup original fallan
exactamente 7 (valores por edge, dot path, índice, no-string, anidado en array,
puertos `cliente`/`vendedor`, no re-escaneo); 8 pasan con ambos y fijan lo que no
cambia (`session_id`, solo-`state`, ausente, choque de nombres, override,
passthrough, `__payload__` desde `config`).

**E2E.** `tests/graphs/basic/input_template_resolution.json` (sin LLM):

| Nodo | Edge | Resultado capturado |
|---|---|---|
| `plantilla` | puertos | `VALOR_PLANO`, `Ana`, UUID de sesión, `""` (ausente), `Ana` (cliente), `Luis` (vendedor) |
| `plantilla_plana` | sin puerto | `VALOR_PLANO`, `Ana`, `{"nombre":"Ana"}`, `""` para `{{origen.plano}}` |
| `sin_puerto` | dos sin puerto | solo `{"rol":"vendedor","nombre":"Luis"}` (ganó el edge posterior; observación, no contrato) |

`dag_engine lint` limpio; `EXPECTED_FILES` 305 → 306. Sin cambio de API ni
wire-format, pero los templates de `input` que antes daban `""` ahora
renderizan su valor.

## 54. Fix: un argumento no declarado por el LLM ya no puede redirigir un valor `fixed` con `${VAR}`

**Esto es un cambio de comportamiento deliberado, no un no-op.** Primera de
una cadena de PRs de seguridad sobre la procedencia de `${VAR}` en el
despacho de tools.

**El hijack.** `merge_args_into_schema()` (usada por CADA llamada de tool con
`node_schema` y por cada fila de `for_each`) templaba `${key}` dentro de los
valores `fixed` del operador contra **todo** el mapa resultante después del
merge — es decir, contra cualquier clave presente ahí, incluyendo un
argumento que el LLM mandó sin que el operador lo hubiera declarado como
parámetro. Un `fixed: "base_url": "${API_BASE}"` se resolvía si el modelo
simplemente mandaba un argumento llamado `API_BASE`, redirigiendo la llamada
a donde el modelo quisiera — sin que el operador hubiera declarado ese
parámetro en ningún lado.

**El fix.** El templado de valores `fixed` ahora corre **antes** del merge, y
solo contra una fuente restringida: los propios valores `fixed` del operador
(uno puede referenciar a otro) más cada parámetro **top-level declarado** que
el llamador haya mandado en esa llamada. Un parámetro anidado dentro de un
contenedor (`param_to_container`) nunca cuenta como fuente. El valor que
manda el LLM nunca se templa a sí mismo — un argumento `q: "${ALGO}"` queda
literal en el resultado.

**Lo que se mantiene igual.** La forma que usa `sql_query` en ADP — un
`query` fijo que referencia `${client_id}`/`${period}` por nombre, ambos
declarados como parámetros top-level — sigue templando exactamente igual;
hay un test de regresión específico para esa forma
(`declared_top_level_param_still_templates_regression_guard` en
`node_schema_merge.rs`). Un `${ENV_VAR}` que nadie declaró como parámetro
queda literal, tal como hoy, a la espera de que el nodo lo resuelva contra el
entorno cuando corra.

**Residual conocido.** Los grafos ya persistidos en la base de datos de ADP
no se pueden auditar desde este repo para confirmar que ninguno dependía de
un argumento no declarado para templar un valor fijo — el análisis de código
fuente en este repo y en `apps/service/ia/platform/` no encontró ningún caso
así, pero es una verificación de código, no de datos en producción.

**Tests.** `node_schema_merge.rs`: caracterización de la forma declarada
(`declared_top_level_param_templates_into_a_fixed_value`), el hijack cerrado
(`undeclared_arg_no_longer_templates_a_fixed_value`), el valor del LLM nunca
se auto-templa (`llm_arg_value_is_never_templated_stays_literal`), el guard
de regresión, y un guard adicional para parámetros anidados en contenedores.
`dag_tool_executor.rs`: un test de integración
(`undeclared_llm_arg_cannot_template_a_fixed_field_through_the_executor`)
prueba que la restricción llega hasta el despacho real de tools, no solo a
la función pura. Mutation check manual (sin `git stash`, copia en el
scratchpad de la sesión): restaurar el templado post-merge sobre todo el
mapa pone en rojo exactamente los dos tests que ejercitan el hijack cerrado
y deja el resto en verde.

**E2E.** `tests/graphs/security/tool_template_source_e2e.json` — un
`llm_call` (Gemini 2.5 Flash) con `http_request` como tool contra
`https://httpbin.org/anything`: `endpoint: "/anything/${bearer_token}"` con
`bearer_token` declarado (un campo real de `http_request`, así que
`dag_engine lint` no reporta nada) y un header `X-Target: "${API_BASE}"` con
`API_BASE` NO declarado en ningún lado del schema. El prompt le pide al
modelo que además mande un argumento `API_BASE: "evil"` que la tool no
describe. Corrido en vivo (`API_BASE=from_env_7c21` en el proceso, un valor
inocuo distinto de "evil", para que el pedido llegue a httpbin en vez de
morir en un error ambiguo) — capture completo en
`/tmp/colmena_e2e/tool_template_source.sse`. El modelo mandó exactamente
`{"bearer_token":"ok","API_BASE":"evil"}`; httpbin devolvió 200 y el echo
prueba las dos mitades a la vez: `"url":
"https://httpbin.org/anything/ok?API_BASE=evil"` (el `bearer_token`
declarado sí templó el `endpoint` a `/anything/ok`; el `API_BASE` no
declarado nunca tocó ningún campo fijo, solo viajó como query param
propio de su argumento) y `"headers": {"Authorization": "Bearer ok", ...,
"X-Target": "from_env_7c21"}` — el header quedó en el valor real del
proceso, NO en `"evil"`: el placeholder `${API_BASE}` nunca se resolvió
contra el argumento del LLM, quedó literal después del merge y lo resolvió
la propia resolución de variables de entorno de `http_request` contra el
entorno real. Antes de este fix, `X-Target` habría llegado como `evil`.

## 55. Fix: se descartan las claves `__colmena*`/`__node*` que el modelo manda como argumentos de tool

Segundo paso de la procedencia de argumentos de tools (el primero fue §54).

**El forjado.** El executor escribe claves de contexto del motor en `inputs`
después de mezclar los argumentos del LLM, con `insert`, así que en general
gana. Pero no siempre las escribe, y en esos huecos una copia forjada por el
modelo llegaba intacta al nodo. Verificado contra el código:

| Clave | La leen | ¿La reescribe el executor en una llamada plana? |
|---|---|---|
| `__colmena_resume_answer` | `llm.rs`, `suspend.rs`, `secure_suspend.rs`, `subgraph.rs`, `orchestrator.rs` | No: solo en un resume real |
| `__node_id` | `llm.rs`, `secure_suspend.rs`, `for_each.rs` | Nunca: solo el loop de grafo la escribe |
| `__colmena_session_id` / `__colmena_agent_session_id` | `llm.rs`, `subgraph.rs`, `secure_suspend.rs`, `tts.rs`, `image_*`, `http.rs`, `document_nodes.rs` | Solo si el executor tiene ese id configurado |
| `__colmena_node_id_path`, `__colmena_subgraph_depth`, `__colmena_tool_name` | `llm.rs`, `subgraph.rs` | Sí, siempre |

Un `__colmena_resume_answer` forjado le hace creer a un `llm_call` o a un
`suspend` usado como tool que hay una respuesta humana que nunca se dio. Dos
caminos no reescribían nada: el despacho de sub-tools de un toolkit y cada
fila de `for_each`, donde el contexto reenviado usaba `or_insert` y una fila
podía ganarle al `session_id` real.

**Fix.** `DagToolExecutor::strip_engine_keys` descarta esas claves de los
argumentos del modelo antes de cualquier merge (`node_schema`, `$DYNAMIC`,
`field_mapping` legado, sin `fixed_config`) y en el despacho de sub-tools de
toolkits. `for_each` limpia cada fila antes del merge y escribe el contexto
reenviado con `insert` después. `subgraph` como tool pasa por el mismo camino;
MCP reenvía los argumentos a un servidor externo sin tocar `inputs` de un nodo;
los toolkits sintéticos no leen esas claves de sus argumentos.

**Tests.** `strips_forged_resume_answer_on_plain_execute`,
`strips_forged_node_id_on_plain_execute`,
`real_resume_answer_wins_over_a_forged_one_in_the_same_call`,
`toolkit_dispatch_strips_forged_engine_keys` (con un sub-tool `inspect` del
toolkit de test `echo_toolkit.rs`), y en `for_each`
`row_supplied_engine_key_is_stripped_not_just_unforwarded` y
`row_supplied_session_id_does_not_override_forwarded_context`. Quitando los
strips y volviendo a `or_insert`, fallan exactamente esos 5 (el de resume real
pasa con los dos códigos).

**E2E.** `tests/graphs/security/tool_strip_engine_keys_e2e.json`: Gemini 2.5
Flash con un `for_each` como tool cuyo destino es `python_script` (no filtra
nada propio, así que es un testigo limpio). El modelo mandó
`{"items":[{"__colmena_session_id":"forged-session","bearer_token":"ok","__node_id":"forged"}]}`.
Lo que vio el `python_script`: `__node_id` ausente, `__colmena_resume_answer`
ausente y `__colmena_session_id` con el UUID real de la sesión, no
`forged-session`. `dag_engine lint` limpio; `EXPECTED_FILES` 307 → 308.

## 56. Provenance de expansión `${VAR}`: se calcula el conjunto de pointers confiables (aún sin gating por nodo)

Tercer paso de la procedencia de argumentos de tools (§54 y §55 lo precedieron).
Módulo nuevo `dag_engine/infrastructure/env_provenance.rs`: `ENV_TRUSTED_PATHS_KEY`
(`__colmena_env_trusted_paths`), `EnvPolicy::{Legacy, Restricted}` con
`from_inputs`/`may_expand`, `trusted_pointers`, `prune_after_secrets`.

**La regla.** `trusted_pointers(authored_fixed, merged)` (§5a-5d de
`22_tool_execution_flow.md`) produce un JSON pointer (RFC 6901) por cada hoja
string que contiene `${` y es idéntica, en el mismo pointer, al valor
autorado. Un objeto nunca es confiable como un todo — solo sus hojas. Un
valor fijo ya templado en §5a puede diferir de su forma autorada para cuando
corre este paso — correctamente no confiable.

**Wiring.** `authored_fixed` sale de `parse_node_schema(schema).fixed_values`
o del `fixed_config` crudo; vacío si no hay ninguno. La clave se escribe
última entre las del motor, después de `strip_engine_keys`. Tras
`inject_secrets`, `prune_after_secrets` descarta cualquier pointer cuyo valor
haya cambiado.

**Aún no cambia el comportamiento de ningún nodo.** Exposición verificada:
`python_node.rs:125` y `trigger.rs:31-34` exponen TODOS los inputs sin
filtrar (ya pre-existente) — la nueva clave solo lleva strings de pointers.
`sse_mapper.rs:689`, `subgraph.rs:74` y `http.rs:254-255` ya filtran por
prefijo sin cambios.

**Tests.** 8 unitarios en `env_provenance.rs` + 3 de executor. Mutación:
forzar `trusted_pointers` a confiar en todo + saltar el prune → 7 tests
fallan; restaurado, vuelven a pasar.

**E2E.** `tests/graphs/agents/sql_read_write_capability_e2e.json`, con una
variante de prompt sin commitear (el original pide un DELETE que el agente
rechaza sin llamar la tool) para forzar un SELECT real. `connection_url`
siguió resolviendo (fila real de `finanzas.gastos`); la captura SSE no
contiene `__colmena_env_trusted_paths` (grep = 0).

## 57. Fix: `http_request` ya no expande `${VAR}` en argumentos escritos por el modelo (requests no multipart)

Cierra §56 para el path NO-multipart. `http.rs` consulta
`EnvPolicy::from_inputs(inputs)` una vez por ejecución; un valor `inputs`
expande `${VAR}` solo si `expand_if_trusted` marca su pointer confiable, un
`config` sigue expandiendo siempre. Gateados: `base_url`, `endpoint`,
`bearer_token`, `authorization`, `headers`/`query_params` hoja por hoja, los
query params extra aplanados, `body` string/objeto recursivo. Un pointer no
confiable sale literal, nunca error. **`execute_multipart` sin cambios**
(sigue sin gate) — alcance del próximo PR.

**Tests.** 4 en `http.rs::env_provenance_gating_tests`. Mutación (gate
siempre permite): 2/4 fallan como se esperaba
(`..._bearer_token_literal_and_never_errors_on_missing_var`,
`..._query_param_header_and_body_leaf_literal`); restaurado, los 47 pasan.

**E2E.** `tests/graphs/security/tool_env_provenance_e2e.json` — header
`fixed` `X-Operator` expande, `bearer_token`/`query_params.probe`
model-authored salen sin resolver. Probe aparece 2 veces, siempre como
header del operador.

**Pendiente.** `execute_multipart` y el resto de los nodos — ver
[13_security_strategy.md](developer_guide/13_security_strategy.md).

## 58. `node-end`/`subgraph-node-end` ganan el wire contract aditivo `status`/`errorText` (sin emisores todavía)

PR 1/4 de "cierre de fronteras en error" (`scratchpad/final_plan_subgraph_error_boundary.md`).
Reporte de ADP: un sub-agente que falla no cierra su nodo en el árbol de UI, así
que la respuesta final del padre queda anidada bajo una rama que nunca terminó.
Este PR es solo el contrato de wire — **nada en el motor construye
`error: Some(..)` todavía**; el primer emisor real (el cierre de una tool
`llm_call`/`for_each`) llega en el PR siguiente de esta serie.

**Wire.** `NodeFinish`/`SubgraphNodeFinish` (`events.rs`) ganan `error:
Option<NodeEndError>` (aditivo, compatible con frames viejos). `SseMapper` lo
traduce a `"status":"error"` + `"errorText"` (si `error.message` es `Some`) en
las cuatro combinaciones `node-end`/`subgraph-node-end` × top-level/wrapped.
Un cierre exitoso sigue sin `status` en absoluto — byte-idéntico a antes. Todo
constructor existente de `NodeFinish`/`SubgraphNodeFinish` pasa `error: None`
para seguir compilando bajo `warnings = "deny"`.

**Tests.** `events.rs` (roundtrip + deserialización legacy),
`sse_mapper.rs` (4: éxito sin `status`, error con/sin `errorText`, variante
wrapped). Sin E2E en este PR — nada emite el campo todavía, así que no hay
frame real que observar; los tests unitarios cubren serialización y mapeo. El
E2E llega en el PR siguiente, cuando `DagToolExecutor` se vuelve el primer
emisor.

**ADP.** Aditivo, sin acción requerida todavía — [nota de
migración](adp_migration/2026-09-15-node-end-error-status.md).

## 59. Fix: un valor seguro descifrado para una tool ya no viaja en texto plano por el stream

**Problema (preexistente, reproducido en vivo).** El resultado que ve el LLM
ya se enmascaraba, pero el stream SSE no. Con `secure_suspend` → handle
`<sv_api_token_…>` pasado a tools: los frames `subgraph-node-start`/`-end` del
hijo de un `subgraph` usado como tool, el `subgraph-node-end` de cierre de un
`for_each` usado como tool y su `batch-item-finished.key` salían con el valor
real. 8 apariciones del secreto en una sola captura.

**Causa.** `DagToolExecutor` descifra los handles en `inputs`, pero
`mask_outbound` solo tocaba el `ToolResult`. El observer que recibe el nodo
despachado no enmascaraba nada, y el frame de cierre se emitía con el resultado
crudo ANTES del bloque de enmascarado.

**Fix.** `MaskingObserver` (`secure_value_service.rs`) serializa cada
`NodeEvent`, reemplaza cada valor descifrado por su handle y lo reenvía; si no
puede enmascarar, descarta el evento en vez de reenviarlo crudo. El executor
envuelve con él el observer del nodo y el del boundary (no-op si la llamada no
descifró nada), y el cierre del boundary ahora sale después de enmascarar.
Genérico a propósito: cubre también campos agregados después (p. ej. el
`error` de los frames de cierre).

**Tests.** `masking_observer_*` (2) y
`tool_dispatch_masks_decrypted_secrets_out_of_stream_events`. Mutaciones (sin
envolver el observer del nodo; cierre crudo antes del enmascarado): ambas
hacen fallar el test.

**E2E.** `tests/graphs/security/secure_value_stream_leak_e2e.json`, dos runs con
el mismo `--agent-session-id`: 0 apariciones del secreto en la captura del run
2 (29 del handle). `es_handle: false` y `largo: 14` prueban que el nodo recibió
el valor real mientras el stream solo lleva el handle.

**Impacto en ADP.** Esos frames traen el handle donde traían el valor; sin
cambio de forma.

**Pendiente.** Los frames de un nodo de grafo normal (no despachado como tool)
seguían sin enmascarar. Cerrado por §61.

## 60. La frontera de una tool `llm_call`/`for_each` es el primer emisor real de `status`/`errorText`

PR 2/4 de "cierre de fronteras en error". §58 landeó solo el contrato de wire;
este PR hace que `DagToolExecutor::execute_inner` lo llene de verdad —
construido sobre el masking unificado que §59 acaba de introducir.

**`DagToolExecutor::execute_inner`.** El cierre de la frontera de una tool
`llm_call`/`for_each` (`scopes_child_events`) ya emitía su `SubgraphNodeFinish`
en éxito y en error, en un único sitio DESPUÉS de `mask_outbound` (§59 lo
movió ahí y lo puso a emitir sobre `masked_boundary_observer`) — eso no es
nuevo. Cambia: ese sitio ahora rama en `Ok`/`Err` del `result` ya enmascarado:
`Ok` sigue con `error: None`; `Err` construye `output: null` y `error:
Some(NodeEndError { message: Some(<texto ya enmascarado por §59> ) })`. La
frontera de `SubGraphNode` y los nodos internos de un run anidado **no** ganan
`status` en este PR — alcance de los PRs 3 y 4.

**Tests.** `dag_tool_executor.rs` (2, nuevo módulo `inner_work_tool_boundary_tests`
con un registro de nodos stub bajo la clave `llm_call`):
`inner_work_tool_failure_closes_boundary_with_error_status`,
`inner_work_tool_success_closes_boundary_without_error_status`.

**E2E.** `tests/graphs/agents/llm_tool_error_boundary.json` (Gemini real, tool
`Helper` → `gemini-does-not-exist-9000`): cierre con `status:"error"` +
`errorText`, antes del `tool-output-available`, sin frame `error`. Éxito:
`tests/graphs/agents/subgraph_tool_basic.json`, sin `status`. Verificador:
`scripts/verify_node_end_status_e2e.py`.

**ADP.** Aditivo — [nota de migración](adp_migration/2026-09-15-node-end-error-status.md)
actualizada; cambio recomendado de una línea en `closeNode(...)`.

## 61. Fix: los frames de un nodo de grafo tampoco llevan valores seguros en texto plano

**Problema (preexistente, reproducido en vivo).** §59 cerró el despacho de
tools; el loop del grafo seguía filtrando. En `secure_suspend_login_direct.json`
resumido, `node-start` de `login` traía el valor real en `inputs`, y `show`
recibía los `<value_N>` de `login` descifrados y los traía reales en
`node-start`/`node-end`.

**Causa.** `execute_stream` descartaba el mapa `(descifrado → handle)` que
devuelve `inject_secrets`, así que no había con qué enmascarar.

**Fix.** `run_secrets` acumula ese mapa en todo el run. Se enmascaran CLONES en
`NodeStart` (`inputs`/`config`), `NodeFinish`/`SubgraphNodeFinish`, el texto de
error del nodo y los eventos de su observer (`MaskingObserver`, como en §59).
El nodo, `all_outputs` y el estado persistido conservan el valor real. El
`GraphFinish` se enmascara solo en el run raíz: el de un run hijo es el retorno
del nodo `subgraph`, bajo una sesión que podría no descifrar el handle.

**Tests.** `tests/secure_values_run_loop_masking.rs` (2, sin DB). Mutación
(sin enmascarar `NodeStart`): ambos fallan. Dos tests con DB
(`secure_value_in_config_integration`, `secure_values_cross_session_integration`)
leían el valor real en `NodeStart.config` —justo la fuga—: ahora el secreto es
el código Python de `show` (`basic/secure_value_in_config_smoke.json` pasó de
`log` a `python_script`), que solo corre si el handle se resolvió. Mutaciones
(inyección en `config` descartada; `config` de `NodeStart` sin enmascarar):
ambos fallan.

**E2E.** `tests/graphs/security/secure_value_run_loop_masking_e2e.json`, sin
LLM, dos runs con el mismo `--agent-session-id`: `probe` hace eco del token y
`strict` recibe ese eco crudo (sin handle) y falla citándolo. Con el loop de
`develop`, el secreto en 4 frames (`probe` start/end, `strict` start, `error`);
con el fix, en 0, y `largo: 14` prueba que `probe` recibió el valor real.
`secure_suspend_login_direct.json` y el grafo de §59: 0.

**Impacto en ADP.** `node-start`, `node-end`, `errorText` y el `finish` raíz
(también el retorno de `run_dag`) traen el handle donde un nodo de grafo ecoaba
un valor descifrado. Sin cambio de forma.

**Pendiente.** El nodo `log` imprime su input descifrado por stdout del proceso
(fuera del stream SSE).

## 62. Fix: la frontera de un `subgraph`-as-tool cierra cuando su child falla (el bug reportado por ADP)

PR 3/4 de "cierre de fronteras en error". Antes: la frontera de una tool
`subgraph` cuyo child fallaba quedaba abierta para siempre. Ver
[guide 19](developer_guide/19_nested_agents_and_subgraphs.md#cuando-el-sub-agente-falla)
y [sse_events_reference.md](sse_events_reference.md#nodo-que-falla) para el
contrato completo — no repetido aquí.

**`SubGraphNode::execute`.** Resuelve el executor antes del `node-start`;
`match` sobre `run_subgraph(...).await` cierra en `Err` antes de re-lanzar,
con `errorText` gateado a `BoundarySource::Tool` (masked por
`MaskingObserver`, #310/#312).

**Tests.** `subgraph_tool_failure_close_tests` (8, TDD red-first — 5 fallan
pre-fix). **E2E.** `subgraph_tool_error_boundary.json`, éxito y SUSPENDED
verificados; `agent>Helper>helper_llm` queda abierto (cerrado en la entrada
63).

**ADP.** [Nota de migración](adp_migration/2026-09-15-subgraph-tool-boundary-closes-on-failure.md).

## 63. Fix: un nodo interno dentro de un run anidado ahora cierra al fallar

PR 4/4 de "cierre de fronteras en error" (cierra la serie). Un nodo que
falla **dentro** de un run anidado ahora cierra a cualquier profundidad
(gap de la entrada 62), sin excepción por `node_type` — un `subgraph`
anidado recibe su propio cierre de loop igual que cualquier nodo, además
del self-close de `SubGraphNode` (#313): dos pares start/end distintos, no
un duplicado (ver PR para el hallazgo). `errorText` reusa el string ya
enmascarado (#312) del `DagError` propagado. Run raíz sin cambios. Tests:
`nested_failure_close_tests` (6, TDD red-first). E2E real:
`subgraph_tool_error_boundary.json` y el nuevo
`edge_wired_subgraph_failure.json` (`inner_sub` balanceado 2/2). Gaps en el
PR: resume sin cobertura unitaria, `for_each` diferido a un PR siguiente.

**ADP.** [Nota de migración](adp_migration/2026-09-15-nested-node-failure-closes.md).

## 64. Fix: un agente con memoria ya no vuelve a contestar la primera pregunta en cada turno

Desde la frontera por interacción (#176), la respuesta a la primera pregunta se
resume a partir del turno 2, pero la pregunta seguía viajando como mensaje `User`
en la cabecera de `SUMMARY_KEEP_FIRST_MSGS` (`llm_call` persiste el turno 1 como
`[User, System]`). Anthropic y Gemini sacan todos los `system` del arreglo, el
resumen incluido, así que el request llevaba la primera pregunta pegada a la abierta,
sin ningún turno del asistente en medio, y el modelo contestaba las dos. Reportado
por ADP en su agente principal del chat: «¿capital de Australia?» volvía a contestar
«¿tasa de la Fed?».

Ahora la cabecera solo deja en el arreglo sus `System`; el objetivo pasa completo al
resumen como `[T0] USER (completo): …` (sin resumir, sin truncar, fuera del tope de
100 líneas). Si la ventana reciente no trae ningún `User`, la cabecera queda entera
como antes. Medido en `gemini-3.5-flash` sobre la conversación real reconstruida
con este código: 5/5 respuestas re-contestaban la primera pregunta antes, 0/5
después. OpenAI (que deja los `system` en su lugar) también recibe el arreglo nuevo.
Tests: 4 nuevos en `history_compaction`. Guía: [§15](developer_guide/15_memory_guide.md).

**ADP.** Sin cambio de contrato; basta con subir el tag.

## 65. Fix: una tool-subgrafo reanudada devuelve su salida, no el estado del hijo

Vale para **todo** subgrafo usado como tool, no solo para `child_graph_ref` (spec
1.7, PR 1/5). La rama de resume de `SubGraphNode::execute` devolvía el `result`
crudo del hijo — el mapa completo de sus nodos, `__colmena_session_id` incluido —
en vez de extraer el nodo marcado `__colmena_is_output_node`, que es lo que el
camino fresco siempre hizo. Un hijo con más de un nodo (p. ej. un `llm_call`
seguido de `output`) filtraba al modelo el output crudo de cada nodo intermedio
en cuanto el usuario contestaba una pregunta de HITL.

**Qué cambió.** Nueva asociada privada `SubGraphNode::extract_final_output(result:
&Value) -> Value`, compartida por las dos ramas: el camino fresco (que ya hacía
esta búsqueda inline) y el de resume (que no la hacía). Además,
`DagToolExecutor::execute_with_resume_answer` ahora pasa su resultado por
`scrub_tool_result_output` antes de devolverlo — el mismo recorte de tamaño que
`ToolExecutor::execute` ya aplicaba en el camino fresco, y que el resume se
saltaba.

**Tests.** `resume_returns_the_output_node_not_the_whole_child_state` (TDD
red-first: fallaba en `assert_eq!(out["output"], json!(42))` porque `out` era el
mapa entero) en `subgraph.rs`, nueva variante de stub `Behavior::ResumeWithOutputs`.
`execute_with_resume_answer_scrubs_the_tool_result_like_execute` (TDD red-first:
la salida contenía los 60 000 caracteres sin recortar) en `dag_tool_executor.rs`,
reusa el armado de `execute_with_resume_answer_threads_value_into_node_inputs` vía
el nuevo `resume_answer_fixture()`.

**E2E.** `tests/graphs/agents/subgraph_tool_hitl.json` (T4: el padre `llm_call`
expone `reservar` como tool-subgrafo; el hijo `sub/suspending_agent.json` tiene dos
nodos, `agent` y `out`, y suspende con `preguntar_usuario`), `gemini-2.5-flash`,
Postgres local. Run 1 (`--agent-session-id`) termina en SUSPENDED; run 2 responde
con `--answer "Q[reserva_num_personas]: … A[reserva_num_personas]: 4 personas"`.
Se lee el mensaje `tool` que el padre guarda en `llm_node_history` (lo que el
modelo relee), con este fix y con `develop` sin él:

| | Claves del resultado de `reservar` al reanudar | Largo |
|---|---|---|
| con el fix | `result`, `extra_info` | 116 |
| `develop` (control) | `agent`, `out`, `__colmena_session_id` | 451 |

Capturas en `/tmp/colmena_e2e/subgraph_resume_output_{fix,base}_{1,2}.sse`.

**ADP.** [Nota de migración](adp_migration/2026-09-23-subgraph-resume-output.md).

## 66. El puerto `ChildGraphResolverPort` y su cableado (sin comportamiento nuevo)

PR 2a/5 de `child_graph_ref` (spec 1.2 y «Ajustes al planificar» 5). Prepara que un
`subgraph` cargue su hijo **por referencia** a través de un puerto que implementa el
embebedor; la resolución en sí llega en la entrada siguiente.

**Qué cambió.** Puerto nuevo en `application/ports.rs`: `ChildGraphResolverPort`
(`resolve(ChildGraphRequest) -> Result<ResolvedChildGraph, ChildGraphResolveError>`),
con cinco motivos de error y `Display` = `CHILD_GRAPH_RESOLVE_FAILED:<code>: <msg>`
(`not_found`, `forbidden`, `needs_config`, `not_runnable`, `unavailable`). Se cablea
como `SubGraphExecutorPort`: un `OnceLock` en `SubGraphNode` que `RouterNode`
comparte, el setter `HashMapNodeRegistry::set_child_graph_resolver` y el campo
`EngineConfig.child_graph_resolver` (`from_env` lo deja en `None`). Las claves de
fuente se mudan a `domain/child_graph_source.rs` y suman `child_graph_ref`: ya queda
fuera del estado del hijo y el catálogo la acepta (tipo `object`), pero **todavía no
se resuelve**: hasta la entrada 67, un subgrafo cuya única fuente es un ref falla con
``Invalid sub-graph JSON: missing field `nodes` `` (medido con el CLI).

**Tests.** Uno nuevo, `within_one_container_inline_and_path_come_before_a_ref`: fija
el orden de `CHILD_GRAPH_SOURCE_KEYS` dentro de un mismo contenedor (reordenar la
constante lo pone rojo). Siguen verdes los 40 de `nodes::subgraph` y los de `registry::` (incluido
`a_migrated_node_config_schema_matches_the_catalog`, que obligó a declarar
`child_graph_ref` en `docs/node_configurations.json`).

**E2E.** Regresión, `gemini-2.5-flash`: `tests/graphs/agents/subgraph_tool_structured.json`
corre igual que antes (3 pares `subgraph-node-start`/`-end`, la tool devuelve el clima)
y el `node-start` del hijo lleva solo `ciudad` y `fecha`: la constante mudada sigue
dejando afuera el plumbing. Captura en
`/tmp/colmena_e2e/child_graph_ref_2a_regression_structured.sse`.

**ADP.** [Nota de migración](adp_migration/2026-09-23-child-graph-ref.md).

## 67. Un `subgraph` carga su hijo por referencia (`child_graph_ref`)

PR 2b/5 de `child_graph_ref` (spec 1.1-1.3). La fuente que la entrada 66 dejó
reservada ahora se resuelve.

**Qué cambió.** `resolve_child_graph_source` devuelve también la clave que encontró
(un ref es un objeto, como un inline). Para `child_graph_ref`, `SubGraphNode` arma un
`ChildGraphRequest` (el `agent_id` templado, el `context` tal cual, la sesión, la
sesión estable y su propia ruta) y le pide el grafo al resolvedor **antes** del frame
de inicio, con un tope de 30 s. Sin resolvedor → `unavailable`; un `agent_id` que
todavía contiene `${` → `not_found` sin consultar al resolvedor. El error vuelve a la
tool como `CHILD_GRAPH_RESOLVE_FAILED:<code>: <msg>`. El grafo resuelto va solo al
ejecutor: no entra en `inputs`, en frames ni en la salida, y queda en
`dag_runs.graph_json` como un inline.

**Tests.** 7 en `child_graph_ref_tests` con un resolvedor doble (TDD: 6 rojos antes
de la implementación; el de precedencia inline>ref ya pasaba). Mutaciones verificadas
en rojo: el grafo en el frame de inicio, el ref en el estado del hijo, el frame de
inicio antes del resolve y el guard de `${`.

**E2E.** `tests/graphs/agents/child_graph_ref_unavailable.json`, `gemini-2.5-flash`,
CLI sin resolvedor: el modelo llama `Run_My_Agent` con `agentId: "agt_demo_42"` y la
tool devuelve `Error executing node Run_My_Agent: CHILD_GRAPH_RESOLVE_FAILED:unavailable:
no child graph resolver configured`. Cero frames `subgraph-*`, y `usage-summary` trae
un solo nodo (el padre). Que el motivo sea `unavailable` y no `not_found` muestra que
`${agentId}` se templó. El camino positivo lo cubren los tests con el doble; su E2E
completo llega con el worker de ADP. Captura en
`/tmp/colmena_e2e/child_graph_ref_unavailable.sse`.

**ADP.** [Nota de migración](adp_migration/2026-09-23-child-graph-ref.md), sección «Desde la entrada 67».

## 68. Router, orquestador y preflight reconocen `child_graph_ref`

PR 3/5 de `child_graph_ref`. La entrada 67 resolvió el ref dentro de
`SubGraphNode`; esta entrada lo hace reconocible en los otros tres sitios que
enumeran las fuentes de un grafo hijo, para que dejen de tratar un ref-only
config como si no tuviera fuente.

**Qué cambió.** `router/config.rs`: la validación de una rama con `subgraph`
ahora cuenta las tres claves de `CHILD_GRAPH_SOURCE_KEYS` en vez de mirar solo
`child_graph_path`/`child_graph_inline`; sin ninguna → «requires
child_graph_path, child_graph_inline or child_graph_ref»; con más de una →
«declares `<las que encontró>` — pick one». `orchestrator.rs`: el chequeo
«Agent must be a subgraph» que antes solo miraba dos claves ahora usa la misma
constante. `preflight.rs`: `enumerate_requirements` salta un `child_graph_ref`
con su propio mensaje («resolved at run time — checked when the subgraph
actually runs») en vez de caer en el genérico «no static child graph source»,
que antes trataba un ref-only config como sin fuente. `tool_configuration.rs`:
sin cambio de comportamiento — `subgraph_inline_child` ya devolvía `None` para
un ref (solo mira `child_graph_inline`) y `memory_backend_missing_reason` ya no
bloquea ese `None` (mismo camino que un `child_graph_path` externo); un test
nuevo lo documenta.

**Tests.** `router::config`: 2 nuevos + 1 actualizado (11 → 13) —
`a_branch_subgraph_may_name_its_child_by_reference` (ok con solo
`child_graph_ref`), `a_branch_subgraph_with_two_sources_is_rejected`
(`child_graph_ref` + `child_graph_inline` → «pick one»),
`subgraph_rejects_neither_path_nor_inline` actualizado al mensaje de tres
claves. `tool_configuration`: 1 nuevo,
`a_dynamic_subgraph_tool_by_reference_is_not_blocked_for_missing_memory` (47 →
48). `orchestrator` (5) y `preflight` (23) sin tests nuevos — el brief no los
pidió; los cubre el E2E de esta entrada.

**Mutación.** El test de memoria ya pasaba antes del PR — documenta
comportamiento existente, así que se verificó que no pase por la razón
equivocada. Mutar `subgraph_inline_child` para que también leyera `child_graph_ref` lo puso
en rojo (devolvía el motivo de bloqueo en vez de `None`); revertido tras
confirmar.

**E2E.** Tres grafos nuevos, `gemini-2.5-flash`, Postgres local. (1)
`tests/graphs/control_flow/router_subgraph_ref_unavailable.json`: una rama
`answerable` con solo `child_graph_ref` ya no se rechaza al cargar (antes:
«requires child_graph_path or child_graph_inline»); el router la elige y falla
en runtime con `router branch 'answerable': CHILD_GRAPH_RESOLVE_FAILED:unavailable:
no child graph resolver configured`. (2)
`tests/graphs/advanced/orchestrator_agent_by_reference_unavailable.json`: el
planner asigna la tarea a `packing_expert` (config con solo `child_graph_ref`,
antes rechazada al validar el agente); el despacho falla con el mismo
`CHILD_GRAPH_RESOLVE_FAILED:unavailable:`. (3)
`tests/graphs/basic/subgraph_ref_only_preflight.json` (sin LLM, prueba solo
preflight): con `RUST_LOG=colmena::preflight=debug` el log muestra
`skipped=["subgraph.child_graph_ref: resolved at run time — checked when the
subgraph actually runs"]` y `covered={}` — no bloquea, no inventa un requisito.
Capturas en
`/tmp/colmena_e2e/child_graph_ref_sites_{router,orchestrator,preflight}.sse`.
`tests/corpus_noise.rs`: 315 → 318 (los tres grafos nuevos).

**ADP.** [Nota de migración](adp_migration/2026-09-23-child-graph-ref.md) — sin
acción nueva; ajustada para describir el estado de punta a punta (router,
orquestador y preflight ya reconocen `child_graph_ref`) y la precisión
`not_found` vs `unavailable` de un `agent_id` sin templar.

## 69. Un `thread_id` fijo da un hilo de memoria por valor, sin exponerlo al modelo

Task 4/5 de `child_graph_ref` (memoria por agente vía `Run My Agent`). Hasta acá,
`memory_mode: "dynamic"` dejaba que el MODELO nombrara el hilo vía `thread_id`
auto-expuesto y requerido; un `node_schema.thread_id` con `fixed` no tenía
tratamiento especial — la plataforma no podía darle a cada agente su propio hilo sin
que el modelo supiera que `thread_id` existe.

**Qué cambió.** Nueva `DagToolExecutor::thread_id_is_fixed(cfg) -> bool` (`pub(crate)`,
usada también por `llm.rs`): verdadera cuando `node_schema.thread_id.fixed` está
presente. La consultan cuatro sitios: `generate_tool_definition` (deja de auto-exponer
`thread_id` si es fijo); el dispatch (antes de sanear, rechaza una plantilla sin
resolver con `ToolResult { success: false, error: Some("unresolved_thread_id") }`,
nunca un hilo compartido); el eco `[hilo: <id>]` (se salta — existe para que el modelo
reuse un id que ÉL inventó); y `list_threads` (`llm.rs::exposes_dynamic_memory` y
`dynamic_tool_names` en `dag_tool_executor.rs`, excluye el tool). La memoria sigue
keyando por el valor resuelto (`tool/<tool_name>/<valor>`): un hilo distinto por
`agentId`.

**Tests.** 6 nuevos en `dag_tool_executor.rs` (80 → 86 funciones de test): los 4 del
brief (`a_fixed_thread_id_is_not_exposed_to_the_model`,
`…_keys_memory_per_value_and_skips_the_thread_prefix`,
`an_unresolved_fixed_thread_id_is_an_error_not_a_shared_thread`,
`list_threads_leaves_out_fixed_thread_tools`) más
`list_threads_dispatch_excludes_fixed_thread_tools`: el cuarto test del brief ejercita
`available_tools()`, que en este archivo **nunca** agregó la tool `list_threads` (ese
gating vive solo en `llm.rs`) — pasaba igual antes del fix, sin probar el filtro de
dispatch que sí cambió. El quinto dispara `list_threads` contra un tool de hilo fijo con
historial real sembrado. TDD red-first en los 4 restantes; el sexto,
`exposes_dynamic_memory_respects_fixed_thread_id`, cierra un hueco de revisión — la
exclusión de hilo fijo en `exposes_dynamic_memory` no tenía test propio, y revertirla no
rompía `nodes::llm` (608) ni `dag_tool_executor::tests` (85).

**Mutación.** `list_threads_leaves_out_fixed_thread_tools` pasaba antes del fix —
revertir el filtro de `list_threads` a mano lo dejó en verde (de ahí el quinto test).
El mismo revert pone en rojo `list_threads_dispatch_excludes_fixed_thread_tools`
(`res.output` lista `archivador`) — revertido tras confirmar.

**E2E.** `tests/graphs/agents/subgraph_fixed_thread_id/` (3 grafos, uno por turno),
`gemini-2.5-flash`, Postgres local, mismo `--agent-session-id`. Turno 1: le dicen a `a1`
un código → llama `archivador` con `{"agentId":"a1","task":"…"}`, sin `thread_id`. Turno
2: `a2` no lo sabe (hilo aislado). Turno 3: `a1` lo recuerda. `llm_node_history` confirma
`tool/archivador/a1/keeper` con turnos 1+3 y `tool/archivador/a2/keeper` aparte con solo
turno 2; mensajes `tool` bajo `chat` empiezan con `{` — cero `[hilo:` en 21 filas ni en 3
capturas SSE (`/tmp/colmena_e2e/fixed_thread_id_turn{1,2,3}.sse`).
`tests/corpus_noise.rs`: 318 → 321.

**ADP.** [Nota de migración](adp_migration/2026-09-23-fixed-thread-id.md) — compilar
"Run My Agent" con `thread_id: { "fixed": "${agentId}" }` en su `node_schema`.

## 70. El nombre del agente en la frontera y la clave en cada entrada de consumo

Task 5/5 (última) de `child_graph_ref`. `ResolvedChildGraph::display_name` (PR
2/5, #317) llegaba hasta `SubGraphNode::execute` como `_display_name` — resuelto,
nunca usado. Y `usage-summary`/`subgraph-usage-summary` no tenían forma de decir
a qué clave de proveedor facturar un nodo, así que el embebedor tenía que
mantener su propio mapeo `node_id → clave` por fuera del grafo.

**Qué cambió.** (1) `SubGraphNode` renombra `_display_name` a `display_name` y lo
mete en el `config` del `NodeStart` de la frontera como `{ "node_label": <nombre>
}`; `SseMapper` lo levanta a un campo de primer nivel del `subgraph-node-start`
envuelto (`config.node_label` → `frame.node_label`). Ninguna variante de
`NodeStart` ganó un campo — es aditivo solo en la salida del mapper. La ruta de
resume sigue sin emitir fronteras, como antes. (2) `llm_call` gana el campo de
config opcional `provider_key_id` (string opaco, no secreto, catalogado en
`docs/node_configurations.json` junto a `api_key`); el motor no lo interpreta,
solo lo repite en la fila de consumo de ese nodo. (3) `node_meta` pasa de una
tupla `(Option<String>, Option<String>, String)` a un struct `NodeMeta { model,
provider, node_type, provider_key_id }`, y el armado de cada fila se extrae a
`usage_entry(node_id, counts, meta: Option<&NodeMeta>) -> Value` (`pub(crate)`,
pura) — el `provider_key_id` se inserta solo si `Some`, nunca como `null`.

**Tests.** 8 nuevos, TDD red-first en los tres
(`only_a_subgraph_boundary_start_is_named_from_its_config` fija que el mapper solo
nombra un inicio `subgraph`, nunca un nodo interno con esa clave en su config):
`nodes::subgraph::child_graph_ref_tests::the_boundary_start_of_a_ref_child_carries_the_agent_name`
(1, reusa el `FakeResolver` existente cuya `Answer::Graph` ya devolvía
`display_name: "Agente de licitaciones"`); en `sse_mapper.rs`,
`a_wrapped_start_lifts_config_node_label_to_the_frame` y su contraparte negativa
`a_wrapped_start_without_config_node_label_carries_no_frame_label` (2, reusan el
helper `wrap()` ya presente en el archivo — no existe `wrapped()`/`map_event()`
por ese nombre); en `run_use_case.rs`, módulo nuevo `usage_entry_tests` con
`a_usage_entry_carries_the_llm_calls_provider_key_id`,
`a_usage_entry_without_a_provider_key_id_omits_the_field` (dos formas de ausencia:
sin `NodeMeta` y con `NodeMeta.provider_key_id: None`),
`a_usage_entry_still_carries_the_pre_existing_fields` (regresión del refactor
tupla→struct) y `a_usage_entry_omits_thinking_tokens_when_zero` (4). `cargo test`
completo: 2772 tests de lib + resto de integración/doctests, 2981 `passed` en
total, 0 `failed`, 74+ ignorados (gateados por `DATABASE_URL`/API en vivo, sin
cambios).

**Mutación.** Las tres piezas revertidas a mano, una por vez: `start_config`
siempre vacío en `subgraph.rs` (el test de frontera se puso en rojo, sin tocar
los otros 48 de ese archivo); el `if let Some(label) = …` borrado en
`sse_mapper.rs` (el test de presencia rojo, el de ausencia se quedó en verde
porque prueba exactamente lo contrario — así confirmado, no vacío); el bloque
`if let Some(key) = meta.provider_key_id` comentado en `usage_entry` (el test de
presencia rojo, el de ausencia y el de campos preexistentes en verde). Además,
sacar `.with_field("provider_key_id", …)` de `llm.rs::config_schema()` sin tocar
el catálogo puso en rojo el test preexistente
`catalog_coverage_tests::a_migrated_node_config_schema_matches_the_catalog` —
esa prueba ya actúa como guarda de mutación para el par código/catálogo. Los
cuatro reverts, restaurados tras confirmar.

**Catálogo.** `llm_call.config_schema()` gana `provider_key_id` (string, no
`required`, sin `valid_values`); `docs/node_configurations.json` →
`llm_call.config_fields.provider_key_id` con la misma forma
(`required: false, default: null`). `cargo run --bin dag_engine -- lint
tests/graphs --fail-on error`: 322 archivos, 0/0/0.

**E2E.** `provider_key_id` — grafo nuevo
`tests/graphs/agents/provider_key_id_usage_e2e.json`
(`gemini-2.5-flash`, dos `llm_call` bajo el mismo trigger: `billed_step` con
`config.provider_key_id: "test-key-123"`, `unbilled_step` sin ese campo).
Corrida real contra `colmena_e2e_cgr` (Postgres local): la fila de
`billed_step` en `usage-summary` trae `"provider_key_id":"test-key-123"`; la de
`unbilled_step` no trae la clave (confirmado con `"provider_key_id" in n` en
Python, no solo lectura visual del JSON). Captura en
`/tmp/colmena_e2e/child_label_and_key_id_provider_key_id.sse`.
`tests/corpus_noise.rs`: 321 → 322. `node_label` **no tiene E2E en este repo** —
la CLI (`dag_engine run`) no tiene `ChildGraphResolverPort` cableado, así que un
`child_graph_ref` nunca arranca ahí; queda cubierto solo por los dos tests
unitarios de arriba. Su verificación end-to-end llega con el worker de ADP, que
sí provee el resolvedor real.

**ADP.** [Nota de migración](adp_migration/2026-09-24-child-label-and-key-id.md)
— leer `node_label` del `subgraph-node-start` de un hijo por referencia; preferir
`provider_key_id` de cada fila de `usage-summary`/`subgraph-usage-summary` al
facturar consumo. Ambos aditivos.

## 71. El alias de un servidor MCP sale de `name`, no de la clave

**Qué cambió.** El alias de un servidor MCP —el prefijo de cada `<alias>__<tool>` que
ve el modelo— era siempre la **clave** de `tool_configurations`, y `name` se ignoraba.
El compilador de ADP del arco MCP vía B (sin mergear al 2026-09-24) indexa esas entradas
por id de nodo (un cuid) y pone el nombre visible en `name`, así que el modelo veía
`cmubmxq86001301s6gpndaze2__ask_question`. Ahora rige el mismo contrato que el resto de
los tipos de tool: `name` si no está en blanco, y si no la clave. Se decide en **un solo lugar** (`collect_mcp_tool_configs`, vía `alias_for`), y
de ahí sale el alias de los nombres expuestos, de las rutas del despachador, de los
bindings, del aviso de servidor no disponible y del campo `alias` de los eventos
`colmena::mcp`. Dos entradas que resuelven al mismo alias no se pisan: la segunda (en
orden de documento) cae a su clave, y si también está tomada, a `<clave>_2`, `_3`…; cada
respaldo se loguea como WARN `mcp.alias_fallback` (`key`, `wanted`, `alias`).

**Compatibilidad.** Un grafo sin `name` en sus entradas `mcp` no cambia: el alias
sigue siendo la clave (`sin_name_el_alias_es_la_clave`, `collect_reads_only_mcp_entries`).
**Sí cambia** un grafo cuya entrada `mcp` trae un `name` no vacío distinto de su clave:
sus nombres expuestos pasan de `<clave>__t` a `<name>__t`, y un system prompt que citaba
los nombres viejos queda desalineado. La referencia decía que `name` se ignoraba y una
versión anterior del motor lo exigía en las entradas MCP, así que grafos fuera del repo
pueden traerlo. Ningún grafo del corpus está en ese caso.

**Tests.** 6 nuevos en `mcp/expose.rs` (alias por `name`, por clave, `name` en blanco,
dos `name` iguales, clave también tomada → sufijo, el WARN del respaldo) y 2 en
`mcp/wire.rs`: uno sigue el alias de punta a punta (definición `deepwiki__ask_question`,
ruta, binding y `McpDispatcher::owns`, sin rastro del cuid) y otro fija que el aviso de
servidor caído nombra el alias y no la clave. Rojo primero: 5 fallaban, las 2 pinzas de
compatibilidad pasaban. Mutación: ignorar `name`, quitar el `trim`, quitar el respaldo a la clave o
todo el respaldo, o el `warn!` del respaldo — cada una pone en rojo su test.

**E2E.** `tests/graphs/agents/mcp_deepwiki_named_e2e.json` (clave cuid, `name:
"deepwiki"`, `enabled_tools: ["deepwiki"]`, la forma de ese compilador) contra DeepWiki
real. Sin clave de LLM en el entorno, se corrió con `COLMENA_PREFLIGHT_HEALTH=off` y una
clave inválida: el cableado MCP corre antes que el modelo, así que lo medido es
`mcp.server_ready alias=deepwiki host=mcp.deepwiki.com tools=3` (antes del cambio, mismo
grafo: `alias=cmubmxq86001301s6gpndaze2`). **No se midió** el lado del modelo —que vea y
llame `deepwiki__ask_question`—: Gemini devolvió `API_KEY_INVALID`.
`tests/corpus_noise.rs`: 322 → 323.

**ADP.** [Nota de migración](adp_migration/2026-09-24-mcp-alias-from-name.md) — ninguna
acción: el compilador del arco MCP vía B (sin mergear al 2026-09-24) ya emite `name`.

## 72. `mcp.tools`: qué tools de un servidor MCP se exponen

**Qué cambió.** `McpServerSpec` gana `tools: Option<Vec<String>>` — los nombres **del
servidor**, verbatim (`resolve-library-id`, no `ctx7__resolve-library-id`). Ausente o
vacía = todas, que es el comportamiento previo: ningún grafo existente cambia; `null`
cuenta como ausente. El compilador de ADP del arco MCP vía B (Startti/adp#808,
mergeado el 2026-09-24) ya emite el campo; el motor lo ignoraba en silencio (el bloque no tiene
`deny_unknown_fields`).

El filtro (`expose::allowed_catalog`) se aplica en `fold_catalog`, el único camino a
`exposed_definitions` —que pasa a `pub(super)`: fuera del módulo no se puede exponer sin
filtrar—, compara nombres **crudos** antes de normalizar y deduplicar, y corre **antes**
del tope de 64 tools por servidor: una tool listada al final de un catálogo grande
sigue entrando (la nota del tope aclara que su conteo es posterior a `tools`). Una tool
que el filtro deja afuera no tiene definición, así que tampoco reclama nombre ni tiene
ruta: `McpDispatcher::owns` da `false`, así que una llamada que el modelo invente cae a
las ramas built-in y se rechaza como tool desconocida —igual que cualquier nombre
inventado— sin llegar al servidor (llamado directo, el despachador responde
`Unrouted`, también sin tocar la red). Una tool listada que el servidor no publica se
reporta al operador como `mcp.wiring_note` (una vez por nombre). Un `tools` que no es
lista de strings falla la carga validada (`Graph::validate`, el linter), y el mensaje lo
nombra entre los campos válidos; si llega en ejecución por `inputs.tool_configurations`,
que no se valida, `collect_mcp_tool_configs` descarta ese servidor entero.

**Tests.** 5 en `mcp/expose.rs` (los 4 del brief: sin `tools` → todas, `[]` → todas,
con `tools` → solo esas, una afuera no llega a definirse y la listada sí; más: la lista
compara contra el nombre del servidor, no el expuesto), 4 en `mcp/wire.rs` (la tool no
listada no tiene ruta, `owns` da `false` y el despachador la rechaza con `Unrouted`; una
listada pasada el tope de 64 se expone; una listada que el servidor no publica se
reporta, deduplicada; con `foo/bar` sin listar antes de `foo.bar` listada,
`srv__foo_bar` va a `foo.bar` y nada va a `foo/bar`) y 1 en `tool_configuration.rs`
(forma inválida de `tools` falla la carga). Rojo primero contra un stub que devolvía el
catálogo entero: 6 + 1 fallaban, las dos pinzas de compatibilidad (ausente, vacía)
pasaban. Mutación: `[]` filtrando todo; toda lista filtrando todo; exponer el catálogo
sin filtrar; filtrar después de normalizar, por nombre expuesto; sin nota; sin
deduplicar — cada una pone en rojo su test.

**E2E.** `tests/graphs/agents/mcp_deepwiki_tools_e2e.json` contra DeepWiki real, con
`tools: ["ask_wiki_question", "no_such_tool"]`. Mismo límite que la entrada 71 (sin clave
de LLM; `COLMENA_PREFLIGHT_HEALTH=off`), así que lo medido es el cableado: de las 3 tools
del catálogo vivo, `mcp.server_ready alias=deepwiki tools=1`, `mcp.tools_exposed
exposed=1` y un `mcp.wiring_note` por `no_such_tool`. **No se midió** el lado del modelo.
La primera corrida listaba `ask_question`, el nombre del fixture de 2026-09-01: DeepWiki
lo renombró a `ask_wiki_question`, y el motor expuso 0 tools y reportó las dos listadas
como no publicadas — el caso para el que existe la nota, observado en vivo.
`tests/corpus_noise.rs`: 323 → 324.

**ADP.** [Nota de migración](adp_migration/2026-09-24-mcp-tools-allowlist.md) — ninguna
acción obligatoria; `[]` significa "todas".
## 73. El esqueleto de un grafo: la regla con que un hijo se va a reanudar (sin comportamiento nuevo)

Task 1/4 de la cadena que hace que un hijo suspendido (`subgraph` por arista, como
tool, de orquestador o de router) se reanude con el grafo que su fuente nombra **en
ese momento** y no con la copia guardada en `dag_runs.graph_json`
(`docs/superpowers/specs/2026-09-24-child-resume-rederive-design.md`, D3). Esta
entrada solo agrega la regla pura de dominio que va a decidir si el grafo fresco
califica para reanudar: nadie la llama todavía.

**Qué cambió.** `domain/graph_skeleton.rs` (nuevo), registrado en `domain/mod.rs`.
`GraphSkeleton::of(&Graph)` reduce un grafo a su **esqueleto**: los ids de nodo con
su `type` (`BTreeMap<String, String>`) y las aristas `(from, to, cyclic)`
(`BTreeSet<(String, String, bool)>`, con `cyclic` ausente = `false`).
`GraphSkeleton::diff(&self, fresh)` devuelve `None` si los dos esqueletos son
iguales, o `Some(SkeletonDiff)` con lo que cambió: `removed`, `added`,
`type_changed` (`(id, tipo guardado, tipo fresco)`) y `edges_changed` (cuántas
aristas están en un esqueleto y no en el otro — una arista que solo cambió
`cyclic` cuenta 2: la plana se va, la cíclica llega). Queda afuera del esqueleto
—y por lo tanto puede cambiar libremente en un resume— todo lo que no sea id/tipo
de nodo o arista: `config` de cada nodo (claves, prompts, rutas de skills),
`timezone`/`location`/`locale` del grafo, `trigger_on` y los topes de llamadas
(`max_total_calls`/`max_calls_from`); `trigger_on` queda afuera porque D3 no lo
nombra y el loop de resume no lo lee. `SkeletonDiff` implementa `Display` con el
texto del rechazo, prefijo `SUBGRAPH_RESUME_INCOMPATIBLE:` (constante pública,
estable como `SUBGRAPH_DEPTH_EXCEEDED:`), que nombra solo ids y tipos — nunca un
valor de `config` — y tapa cada lista en 5 ítems con `+N more`. Nadie llama esta
regla todavía: la entrada 74 la usa desde el ejecutor del resume
(`resume_subgraph`), y un PR posterior la aplica de punta a punta en
`SubGraphNode` (el que hace que `subgraph.rs` pase `Fresh` en vez de `Stored`).

**Tests.** 8 nuevos, TDD red-first (Step 1 no compilaba: `GraphSkeleton`/
`SkeletonDiff` no existían; Step 2 los deja en verde): `the_same_graph_has_no_diff`;
`config_graph_context_and_limits_may_change_freely` (una `api_key`, un
`system_message`, una ruta de skills, `max_total_calls` y el `timezone`/`locale`
del grafo cambian sin producir diff); `an_absent_cyclic_flag_is_false`;
`a_removed_or_added_node_is_named`; `a_changed_type_names_both_types`;
`edges_differ_by_endpoints_and_by_cyclic` (agregar, quitar y voltear `cyclic`, las
tres formas); `the_message_leads_with_the_prefix_and_names_ids_and_types_only`
(siembra `sk-must-not-leak` en la config del nodo que cambia de tipo y confirma
que el texto no la contiene); `long_lists_are_capped`. `cargo test -p
colmena_dag_engine --lib`: 2773 → 2781 passed (74 ignorados sin cambio, 0 failed);
filtrado a `graph_skeleton`: 8 passed.

**Mutación.** Las 4 del plan, cada una en rojo y revertida a mano: (1) meter la
`config` del nodo en el valor que guarda `nodes` (`format!("{}{}", node_type,
config)` en vez de solo `node_type.clone()`) puso en rojo
`config_graph_context_and_limits_may_change_freely` (además, de arrastre,
`a_changed_type_names_both_types` y `the_message_leads_with_the_prefix…`,
porque la config viajaba disfrazada de tipo); (2) forzar el tercer elemento de la
tupla de arista a `false` siempre (`cyclic` deja de importar) puso en rojo
`edges_differ_by_endpoints_and_by_cyclic` (el caso `cyclic`: `Some(2)` → `None`),
sola; (3) `e.cyclic.unwrap_or(true)` puso en rojo `an_absent_cyclic_flag_is_false`
(además `edges_differ_by_endpoints_and_by_cyclic`, de arrastre); (4) `SkeletonDiff::fmt`
agregando `{:?}` de `self` al mensaje puso en rojo, sola,
`the_message_leads_with_the_prefix_and_names_ids_and_types_only`. Las cuatro,
revertidas tras confirmar; `cargo test --lib graph_skeleton` vuelve a 8 passed.

**E2E.** No aplica: no hay comportamiento observable hasta que `subgraph.rs`
pase `Fresh` en producción (PR posterior).

**ADP.** Sin nota: nada cruza la frontera todavía; la nota llega con la entrada 74.

## 74. El puerto de resume lleva el grafo con que reanudar (sin comportamiento nuevo)

Task 2/4 de la cadena (`docs/superpowers/specs/2026-09-24-child-resume-rederive-design.md`,
D3). Conecta la regla de la entrada 73 (`GraphSkeleton`) al ejecutor del resume;
`SubGraphNode` sigue pasando `ResumeGraph::Stored` en todo resume — **sin
comportamiento observable todavía**, eso llega con un PR posterior, el que
hace que `subgraph.rs` pase `Fresh`.

**Qué cambió.** `SubGraphExecutorPort::resume_subgraph` gana `graph: ResumeGraph`
(`application/ports.rs`): `Fresh(Value)` (grafo re-derivado, secretos ya
resueltos, `Debug` redactado a mano), `Unavailable(String)` (la fuente no pudo
dar uno) y `Stored` (comportamiento de hoy). `DagError` suma
`ResumeRefused(String)` con `#[error("{0}")]` — sin el prefijo `"Error de
ejecución en el nodo: "` de `NodeExecution`, para que
`SUBGRAPH_RESUME_INCOMPATIBLE:`/`CHILD_GRAPH_RESOLVE_FAILED:` sigan liderando
`ToolResult.error` también en el resume. `DagRunUseCase::plan_resume`
(`run_use_case.rs`) decide sin correr nada: `Stored` corre el guardado
(ilegible sigue siendo `Invalid sub-graph state JSON: …`, sin cerrar);
`Unavailable` se rechaza tal cual; para `Fresh` el guardado se parsea
**primero** — así gana su error si los dos están rotos, no el rechazo que
cerraría la fila — y recién entonces se mira `fresh`: si no parsea, texto fijo
(nunca el error de serde, que puede citar un valor del grafo); si sí, compara
esqueletos (`GraphSkeleton::of(&stored).diff(&GraphSkeleton::of(&fresh))`) y
corre el fresco si calzan, o rechaza con el `SkeletonDiff`; uno que calza pero
falla `Graph::validate()` (fuera de `plan_resume`) también queda sin cerrar.
Un rechazo cierra la fila `FAILED` vía `close_refused` (el grafo que ya tenía —
el fresco nunca llega a `save`) para que un turno siguiente no la retome como
`SUSPENDED`; `resume_subgraph` llama `plan_resume` y devuelve
`DagError::ResumeRefused` sin tocar el stream ante un rechazo. `subgraph.rs`
pasa `ResumeGraph::Stored` en su único call site de producción (`:396-405`);
los tres dobles de test reciben el parámetro nuevo sin usarlo.

**Tests.** 9 en total (eran 5). Nuevos, en `run_use_case.rs::resume_graph_tests`:
`a_fresh_graph_that_fails_to_parse_is_refused_without_leaking_the_bad_value`
(un `max_total_calls` no numérico con `"sk-fresh-secret"` — el rechazo lleva
el prefijo y nunca el valor);
`an_unparsable_stored_graph_is_todays_error_and_does_not_close_the_row` y
`a_fresh_graph_that_fails_validation_does_not_close_the_row` (los dos casos
sin cierre del ajuste 4, ambos `Suspended`, nunca `SUBGRAPH_RESUME_INCOMPATIBLE:`);
`when_both_graphs_are_unparsable_the_stored_failure_wins_and_nothing_closes`
(pin del orden de parseo). `a_changed_skeleton_is_refused…` ganó 3 asserts
(`agent_session_id`/`parent_session_id`/`active_queue`): `close_refused` solo
toca `status`. `cargo test -p colmena_dag_engine --lib`: 2817 passed (0
failed, 74 ignorados — la rama recibió `## 76.` entre la 74 y esta revisión);
filtrado a `resume_graph`: 9 passed; a `nodes::subgraph`: 49 passed; a
`ports`: 21 passed, 1 ignorado.

**Mutación.** Las 5 originales más dos de este seguimiento, en rojo y
revertidas a mano: (6) `Err(_) => …` a `Err(e) => …{e}` puso en rojo, sola,
`a_fresh_graph_that_fails_to_parse_is_refused_without_leaking_the_bad_value`
(el secreto sembrado aparece en el mensaje); (7) deshacer el reordenamiento
(parsear `fresh` antes que `stored`) puso en rojo, sola,
`when_both_graphs_are_unparsable_the_stored_failure_wins_and_nothing_closes`.
`cargo test --lib resume_graph` vuelve a 9 passed tras cada revert.

**E2E.** Regresión: sin comportamiento nuevo, pero la firma de `resume_subgraph`
cambió y todo resume pasa por ella. Corrida real contra `colmena_e2e_cgr`
(Postgres local) con `tests/graphs/basic/suspend_in_subgraph.json`: turno 1
(`subgraph_resume_2a_regression_1.sse`) suspende con
`finishReason: "suspended"`; turno 2, con la respuesta a `confirm_transfer`
(`subgraph_resume_2a_regression_2.sse`), resume y termina con
`finishReason: "stop"`. Cero frames `error` en ninguno de los dos. La fila del
hijo (`agent_session_id`/`parent_session_id is not null`) queda `COMPLETED`.

**ADP.** [Nota de migración](adp_migration/2026-09-24-subgraph-resume-fresh-graph.md)
(índice actualizado en `docs/adp_migration/README.md`): ningún cambio de código
— ADP no implementa `SubGraphExecutorPort` — pero el agente principal y la
descripción de `Run My Agent` necesitan traducir `SUBGRAPH_RESUME_INCOMPATIBLE:`
cuando `subgraph.rs` empiece a pasar `Fresh` (PR posterior).

## 75. Un rechazo de resume cierra los descendientes SUSPENDED del hijo, y el cierre es atómico

Ajuste de revisión sobre la entrada 74
(`docs/superpowers/specs/2026-09-24-child-resume-rederive-design.md`, D4):
`close_refused` cerraba solo la fila del hijo rechazado. En una cadena raíz →
A → B (B es quien suspendió, lo que deja A también SUSPENDED), si el resume de
A se rechaza, B se quedaba SUSPENDED bajo un padre ya FAILED —
`find_resume_entry` lo cuenta como cadena propia ("Found N concurrent suspended
chains") o elige una hoja obsoleta. Además `close_refused` era un
read-modify-write de la fila completa: sin guarda SUSPENDED (podía voltear una
fila COMPLETED) y con el commit de un escritor concurrente perdible entre el
`get_by_id` y el `save`. Todavía latente —todo caller pasa `Stored` y nadie
rechaza— hasta que un PR posterior haga que `subgraph.rs` pase `Fresh`; esta
entrada tiene que llegar antes.

**Qué cambió.** `DagStateRepository` (`domain/state.rs`) gana dos métodos con
impl por defecto, mismo patrón que `cancel_running_descendants`:
`fail_if_suspended` (compone `get_by_id`+`save` en el default) y
`fail_suspended_descendants` (default `Ok(0)`, no-op aceptable para repos en
memoria/test). `PostgresDagStateRepository`: `fail_if_suspended` es un único
`UPDATE ... WHERE session_id = $1 AND status = 'SUSPENDED'`, atómico;
`fail_suspended_descendants` reusa el CTE recursivo de
`cancel_running_descendants` para voltear a FAILED cada descendiente todavía
SUSPENDED (la fila misma queda afuera). `close_refused` (`run_use_case.rs`)
llama primero `fail_if_suspended` y, solo si volteó la fila, a
`fail_suspended_descendants`; si la fila ya no estaba SUSPENDED (cerrada por
otro escritor, o terminal por otra razón) no toca nada más —no es su fila para
cerrar, ni sus descendientes—. El texto del rechazo no cambia.

**Tests.** 2 nuevos en `run_use_case.rs::resume_graph_tests` (11 en total, eran
9): `a_refused_resume_closes_its_suspended_descendants_but_not_unrelated_rows`
(cadena hijo→nieto, más una fila padre y una de otra cadena, ambas intactas) y
`fail_if_suspended_leaves_a_completed_row_untouched` (contra el default del
trait). `MemRepo` gana `fail_suspended_descendants` (recorrido transitivo real
por `parent_session_id`); `fail_if_suspended` queda en el default del trait.
Integración nueva, `#[ignore]`, `tests/resume_refuse_descendants.rs`: siembra
root(SUSPENDED)→a(SUSPENDED)→b(SUSPENDED)→c(COMPLETED) más una fila SUSPENDED
de otro chat; cierra `a`, confirma `a`/`b` en FAILED y `c`/root/la fila ajena
intactos, y que `find_resume_entry` devuelve `root`, no `b`; un segundo test
fija la guarda SUSPENDED de `fail_if_suspended` contra una fila COMPLETED.
`cargo test -p colmena_dag_engine --lib`: 2819 passed (0 failed, 74 ignorados,
+2 sobre la entrada 74). `cargo test` completo (workspace): 3028 passed, 0
failed, 144 ignorados.

**Mutación.** 3, cada una en rojo y revertida a mano: (1) comentar la llamada a
`fail_suspended_descendants` en `close_refused` → rojo, sola,
`a_refused_resume_closes_its_suspended_descendants_but_not_unrelated_rows`; (2)
quitar `AND status = 'SUSPENDED'` de cada UPDATE de Postgres, uno por vez →
rojo, cada vez sola, en la integración (el de `fail_if_suspended` puso en rojo
`fail_if_suspended_leaves_a_completed_row_untouched`; el del CTE puso en rojo
`refusing_a_child_closes_its_suspended_descendants…`, que detectó a `c`,
COMPLETED, volteada también); (3) el default de `fail_if_suspended` ignorando
el guard de status → rojo, sola, la versión unitaria de
`fail_if_suspended_leaves_a_completed_row_untouched`.

**E2E.** Nada en producción rechaza todavía (todo caller pasa `Stored`), así
que no hay corrida observable por SSE —eso llega con el PR que hace que
`subgraph.rs` pase `Fresh`—. La evidencia end-to-end de esta entrada es la
integración de Postgres de arriba, corrida real contra `colmena_e2e_cgr`
(Postgres local, `--ignored`): 2 passed, 0 failed.

**ADP.** [Nota de migración](adp_migration/2026-09-24-subgraph-resume-fresh-graph.md)
actualizada: un rechazo también cierra los descendientes SUSPENDED del hijo, y
`DagStateRepository` gana dos métodos con impl por defecto —no rompe a quien
implemente el puerto fuera del crate—.

## 76. El cliente MCP no marca direcciones que no sean públicas

**Qué cambió.** El motor se niega a conectar un servidor MCP en una dirección que no sea unicast
global — la tabla de `isGlobalUnicast` de ADP (`safeFetch`): IPv4 fuera de los 15 prefijos de IANA;
IPv6 solo `2000::/3` menos `2001::/23`, `2001:db8::/32` y `2002::/16` (cae toda IPv4 mapeada y
NAT64). Una dirección no pública, o ninguna, rechaza la respuesta DNS entera. **Encendida por
defecto**; `COLMENA_MCP_ALLOW_PRIVATE_HOSTS` = `1`/`true` la apaga (se lee una vez por proceso),
solo para desarrollo local. Producción no debe fijarla.

**Dónde decide.** `RmcpHttpClient` arma su propio `reqwest::Client` (`with_client`) cuyo
`dns::Resolve` devuelve exactamente lo que filtró: la IP chequeada es la IP marcada. Un host IP
literal se chequea PARSEADO (`0177.0.0.1` → `127.0.0.1`). Sin redirecciones, sin pool ocioso (cada
request re-resuelve) y `.no_proxy()`. Al cablear, el modelo solo recibe el aviso «did not respond» y
`destination is not a public address` va a `mcp.wiring_note`; lo lee solo si lo rechaza una
reconexión en el dispatch. La dirección va únicamente a `mcp.dial_refused`.

**Verificación.** 9 tests nuevos; 11 mutaciones, cada una roja en su test (una apaga la guarda en
el `connect` de producción). E2E sin clave de LLM: deepwiki `server_ready` y `mcp.dial_refused`
para `https://localhost/mcp` y `https://169.254.169.254/mcp`; con `HTTPS_PROXY` fijado DeepWiki
responde y sin `.no_proxy()` cae. [Nota](adp_migration/2026-09-24-mcp-private-dial-guard.md).

## 77. `SubGraphNode` carga su grafo hijo con una sola función

Task 3/4 de la cadena (`docs/superpowers/specs/2026-09-24-child-resume-rederive-design.md`,
D3), primer paso: un refactor puro, sin comportamiento observable. Prepara el
terreno para que un resume pueda derivar su grafo con la misma carga que el
camino fresco — la entrada siguiente conecta eso.

**Qué cambió.** La carga del camino fresco (`config`/`inputs` → resolver un
`child_graph_ref`, tomar un `child_graph_inline` tal cual, o releer un
`child_graph_path`) sale de `execute()` a `load_child_graph`, un método nuevo de
`SubGraphNode`. El camino fresco es su único llamador por ahora; el mismo texto
de error en cada rama, el mismo orden de chequeo (`ref` → `inline` → `path`). La
rama de resume no cambia: sigue pasando `ResumeGraph::Stored` en su único call
site de producción.

**Tests.** Ninguno nuevo — es una extracción de método, cubierta por los tests
existentes del camino fresco (`child_graph_ref_tests`, 8; `subgraph_tool_input_config_tests`,
6; `subgraph_child_state_isolation_tests`, 8; y el resto de `nodes::subgraph`).
`cargo test -p colmena_dag_engine --lib nodes::subgraph`: 49 passed (sin cambio),
0 failed.

**Mutación.** 1, en rojo y revertida a mano: invertir la condición
`source_key == CHILD_GRAPH_REF` de `load_child_graph` a `!=` — 23 tests caen (todo
lo que pasa por el camino fresco: los 8 de `child_graph_ref_tests`, los de
`subgraph_as_tool_boundary_tests` y `subgraph_tool_failure_close_tests`),
confirmando que la extracción no cambió en silencio el orden de las ramas.
Revertida; `cargo test --lib nodes::subgraph` vuelve a 49 passed.

**E2E.** Regresión: sin comportamiento nuevo, pero el único llamador de
`load_child_graph` es el camino fresco y su firma interna cambió. Corridas reales
contra `colmena_e2e_cgr` (Postgres local): `tests/graphs/agents/child_graph_ref_unavailable.json`
(sin resolvedor configurado) — 1 `tool-output-available` con
`CHILD_GRAPH_RESOLVE_FAILED:unavailable: no child graph resolver configured`,
igual que la entrada 67; `tests/graphs/basic/suspend_in_subgraph.json` (un
`child_graph_inline` fresco) — `finish.finishReason == "suspended"`, llega al
`suspend` interno del hijo sin error.

**ADP.** Sin nota: nada cruza la frontera todavía — mismo texto de error, mismo
comportamiento observable, solo se movió código dentro del motor.

## 78. Un subgrafo reanudado corre el grafo que su fuente nombra hoy

Task 3/4 de la cadena (`docs/superpowers/specs/2026-09-24-child-resume-rederive-design.md`,
D3), segundo paso: conecta las entradas 73-75 y 77. `SubGraphNode` deja de pasar
`ResumeGraph::Stored` en todo resume — **comportamiento observable**.

**Qué cambió.** `resume_graph` (nuevo) decide qué `ResumeGraph` pasarle al
ejecutor: sin válvula y con fuente, deriva con `load_child_graph` (`Fresh`) o
falla (`Unavailable`, path inexistente o JSON roto); sin fuente,
`Unavailable` con `SUBGRAPH_RESUME_INCOMPATIBLE:` adelante (entrada 73, primera
vez que dispara en producción); un `child_graph_ref` sigue devolviendo `Stored`
(el resolvedor no se vuelve a llamar todavía, PR posterior). La rama de resume
pasó de "buscar el hijo → `resume_subgraph(Stored)`" a "buscar el hijo → derivar
→ `resume_subgraph(ResumeGraph)`". Válvula `COLMENA_SUBGRAPH_RESUME_GRAPH=stored`
(cualquier otro valor deriva), leída una vez por proceso vía `OnceLock` como
`COLMENA_MAX_SUBGRAPH_DEPTH`, con su mitad pura (`valve_is_stored`) separada.
Texto por tool: `ToolResult.error` empieza con el prefijo, `output` = `Error
executing node <tool>: …`; por arista u orquestador: `Error de ejecución en
el nodo: SUBGRAPH_RESUME_INCOMPATIBLE: …`. Los dos cierran la fila del hijo (y
sus descendientes SUSPENDED, entrada 75) como `FAILED`. Ningún frame SSE nuevo.
Por router, con `router branch '<rama>': ` delante (`router/node.rs:160`): vuelve a elegir su rama en cada resume, y una del mismo esqueleto reanuda con su config.

**Tests.** 9 nuevos en `subgraph.rs` (TDD red-first): 8 en
`subgraph_resume_graph_tests` (inline/path derivados en resume, path inexistente,
sin fuente, sin hijo suspendido, válvula, `valve_is_stored`) + 1 en
`child_graph_ref_tests` (`a_ref_still_resumes_its_stored_graph`). `cargo test
--lib nodes::subgraph`: 49 → 58 passed; `router`/`orchestrator` sin regresiones
(35/5 passed).

**Mutación.** 3, cada una en rojo (o confirmada equivalente) y revertida: (1)
`resume_graph` siempre `Stored` → 5 rojos (no solo los 3 de inline/path: también
los 2 casos `Unavailable`, porque ya no fallan donde antes fallaban); (2) derivar
antes de buscar el hijo → sin cambios, equivalente a este nivel (derivar un
inline no pasa por el executor mockeado; la Task 4 de la cadena sí lo detecta,
porque ahí derivar es una llamada al resolvedor); (3) `valve_is_stored` sin
`trim`/case-insensitive → rojo, solo, el caso `" STORED "`.

**E2E.** `tests/graphs/advanced/subgraph_resume_fresh_graph/turn1_suspend.json`
(nuevo; los dos turnos 2 se derivan con `jq` en el momento, ver su README — no se
commitean). `EXPECTED_FILES` 324 → 325. Lint: 1 file, 0/0/0; corpus 325 files,
0/0/0. Corridas reales contra `colmena_e2e_cgr`, esta rama (`new`) y
`colmena_dag_engine-v0.16.0` (`old`): config nueva → `new`→`new` da `SELLO=v2`,
`old`→`old` da `SELLO=v1` (confirma que el arreglo es lo que cambió); válvula
(`new`→`new` con `COLMENA_SUBGRAPH_RESUME_GRAPH=stored`) → `SELLO=v1`; estructura
nueva → `SUBGRAPH_RESUME_INCOMPATIBLE: … (removed: pregunta; added: confirmar;
edges changed: 4)`, fila hija `FAILED`, sin `subgraph-node-end` de `sello`;
compat (`old`→`new`) → `SELLO=v2`, fila hija `COMPLETED` — un run suspendido por
v0.16 se reanuda fresco sin migración. Los cuatro turnos 1: `suspended`. Además,
un nieto anidado a dos niveles con LLM real (`nested_resume_liveness_e2e.json`):
1 salida de tool con un `sello` inyectado en el turno 2 en la rama del PR, 0 en
la línea base v0.16.0; el `subgraph-usage-summary` del resume nombra
`db_specialist_agent` (no `usage-summary`, que solo lleva el nodo raíz).
Capturas en `/tmp/colmena_e2e/`: `final_{cfg,base,valve,shape,compat}_{1,2}.sse` y `subgraph_resume_nested{,_base}_{1,2}.sse`.

**ADP.** [Nota de migración](adp_migration/2026-09-24-subgraph-resume-fresh-graph.md)
actualizada.

## 79. Fix: un argumento del modelo ya no elige el grafo que corre un `subgraph`

**El agujero.** Un `subgraph` como tool lee su fuente de `inputs` (inline > path >
ref), adonde un argumento no declarado llegaba intacto. Un `child_graph_inline` o
`child_graph_path` agregado a la llamada le ganaba al `child_graph_ref` fijo de
"Run My Agent" —sin consultar al resolvedor— o al path fijo de una tool legada, y
el worker corría el grafo del modelo: `python_script` sin sandbox por defecto
(`python_node.rs:213-217`), `${VAR}` del entorno en todo `config` (`http.rs:492`,
`llm.rs:1225`, `sql.rs:115`), cualquier archivo del disco por path
(`subgraph.rs:141-145`). El asset (inline fijo) ya estaba a salvo por precedencia.

**Qué cambió.** `drop_unoffered_child_graph_sources` (`node_schema_merge.rs`) saca
de los argumentos toda clave de `CHILD_GRAPH_SOURCE_KEYS` que la tool no ofrezca
como parámetro (los de la definición que recibió el modelo; en `for_each`, los
campos visibles del target), con un aviso sin valores. Corre junto a
`strip_engine_keys`, antes de las cuatro estrategias de merge —dentro de
`merge_args_into_schema` solo cubriría `node_schema`— y en cada fila de
`for_each`. Una fuente declarada pasa (`probar_grafo` del graph builder). No se
extiende a toda clave de configuración: filas de `for_each`, query params extra
de `http_request`, globals de `python_script` y estado del hijo dependen hoy de
que un argumento no declarado pase.

**Tests.** 6 en `child_graph_source_arg_tests` (`dag_tool_executor.rs`), con un
`SubGraphNode` real: 4 rojos antes del fix (inline y path contra ref fijo, inline
contra path legado, `subgraph` por nombre crudo) y 2 guardas verdes en ambos lados
(asset en sus dos formas, fuente declarada); 1 en `for_each.rs`, rojo antes.
`EXPECTED_FILES` 324 → 325. `cargo test` completo: 3035 passed, 0 failed, 144 ignorados.

**Mutación.** 5, rojas y revertidas a mano: sin la llamada del executor → los 4
repros; sin la de `for_each` → su test; ignorar `offered` → las 2 guardas
declaradas; lista a mano sin `child_graph_path` → el repro de path; el nombre
crudo ofreciendo `child_graph_inline` → el repro crudo.

**E2E.** `tests/graphs/security/tool_child_graph_source_e2e.json` por el CLI
contra un stub local de `generateContent` (`GEMINI_BASE_URL`; llamadas exactas,
sin modelo real), con `COLMENA_E2E_CANARY=canary_4f1e`. El modelo manda un grafo
cuyo `python_script` lee la canaria: inline y por archivo a `Run_My_Agent`, inline
a `especialista` (path legado) y a `probar_grafo` (lo declara). Sin el fix: 4
`subgraph-node-start` de `pwn`, 4 `tool-output-available` con la canaria. Con el
fix: 1 (`agent>probar_grafo>pwn`); `Run_My_Agent` da 2 veces
`CHILD_GRAPH_RESOLVE_FAILED:unavailable` (no `not_found`: `agentId` templó);
`especialista` corre su archivo; 3 avisos en stderr sin la canaria. Los
`functionResponse` del stub coinciden. Capturas en
`/tmp/colmena_e2e/tool_child_graph_source_{before,after}.sse`.

**Fuera de alcance.** El despacho no compara el nombre de la tool con las
expuestas: con el mismo stub, un `python_script` no expuesto corrió y leyó la
canaria. Lo cierra la entrada 80.

**ADP.** [Nota](adp_migration/2026-09-24-tool-args-cannot-set-child-graph.md):
ninguna acción de código; subir el motor.

## 80. Fix: el modelo solo corre las tools que el request le ofreció

**El agujero.** El loop del agente le pasaba al executor cualquier nombre que el
modelo emitiera, y `DagToolExecutor::execute_inner` cae al registro para un nombre
que no es tool configurada y despacha las sintéticas por nombre sin mirar si se
ofrecieron (`gsheets_*`, `gdocs_*`, `data_run_python`, `api_explorer__*`…).
Ningún adapter (Gemini, OpenAI, Anthropic) compara el nombre devuelto con las tools
declaradas. Con un stub del proveedor, un `llm_call` que ofrecía otras tools corrió
un `python_script` que leyó una canaria del entorno. Es el «Fuera de alcance» de la 79.

**Qué cambió.** `agent_service.rs:497` corre una llamada solo si su nombre está en
`iteration_tools`, la lista que ese request serializó para el proveedor: no es una
segunda lista y no puede divergir (declaradas, `enabled_tools`, sintéticas del motor,
MCP, lazy ya cargadas). Si no, `Error executing tool: Tool not found: <nombre>` —el
texto de un nombre inexistente: ni pista del registro ni eco de los argumentos— y un
WARN `tool.not_offered` con nombre y `tool_call_id`. El rechazo se persiste como tool
message, así que nunca es la llamada pendiente de un resume. Lazy conserva sus reglas:
una tool del catálogo no cargada devuelve su schema, y `describe_tool` responde aunque
la lista ya no lo traiga. En el loop y no en el executor: el executor se construye
antes de armar la lista (y lazy la cambia por iteración); el loop es el único camino de
una llamada del modelo. El replay de resume repite una llamada que ya pasó la guarda
(salvo una historia suspendida por una versión anterior).

**Tests.** 3 en `agent_service.rs` (el repro, rojo antes; tools ofrecidas declarada, del
motor y con forma MCP; el flujo lazy) y 1 en `registry.rs` con el `llm_call` real y un
modelo guionado: `multiply`, registrado y no ofrecido, devolvía `{"output":6.0}`; ahora
`Tool not found`, y `sumar` (declarada) y `recall_history` (del motor) corren. 13 tests
del loop y 2 de integración pasaban `tools: vec![]` y llamaban tools igual: ahora
declaran lo que llaman. `cargo test` completo: 3041 passed, 0 failed, 144 ignorados.

**Mutación.** 3, rojas y revertidas a mano: sin el rechazo → el repro y el de
`registry.rs`; sin la excepción de `describe_tool` → el test lazy; la lista estática
`tools` en vez de `iteration_tools` → el test lazy (la tool del catálogo corre a ciegas).

**E2E.** `tests/graphs/security/tool_unoffered_dispatch_e2e.json` por el CLI contra un
stub local de `generateContent` (`GEMINI_BASE_URL`), con `COLMENA_E2E_CANARY`. El
request ofrece `["sumar","recall_history"]`; el modelo llama `python_script`,
`api_explorer__list_endpoints`, `sumar` y `recall_history`. Sin el fix: la canaria en el
SSE y en el `functionResponse`, y `api_explorer` corre. Con el fix: las dos primeras
`Tool not found`, 0 canarias, 2 WARN; `sumar` = `{"output":5.0}` y `recall_history`
responde en los dos. Variante lazy (derivada con `jq`): redirect, `describe_tool`
responde el schema, `sumar` corre, `python_script` rechazado. `EXPECTED_FILES` +1; lint
0/0/0. Capturas: `/tmp/colmena_e2e/tool_unoffered_dispatch{,_lazy}_{before,after}.sse`.

**ADP.** [Nota](adp_migration/2026-09-24-unoffered-tool-refused.md): ninguna acción de
código; subir el motor.

## 81. Un `child_graph_ref` se vuelve a resolver al reanudar

Task 4/4 de la cadena (`docs/superpowers/specs/2026-09-24-child-resume-rederive-design.md`,
D3), último paso: conecta la excepción que dejó la entrada 78. `resume_graph`
deja de devolver `ResumeGraph::Stored` para un `child_graph_ref` — **comportamiento
observable**.

**Qué cambió.** Una línea de código: el bloque que hacía `if source.0 ==
CHILD_GRAPH_REF { return ResumeGraph::Stored; }` en `resume_graph` desaparece, así
que un ref cae en la misma rama que inline/path y llama a `load_child_graph`, que
ya sabía resolver un ref (la usa el camino fresco desde la entrada 67). El
resolvedor recibe el mismo `ChildGraphRequest` que armaría una corrida fresca
(`agent_id` ya templado, `context`, sesión, sesión estable, `parent_path`), y
solo **después** de que `find_child_session_id_for_resume` encontró un hijo
suspendido — sin hijo, no hay resolve, y en ADP eso importa: cada resolve acuña
un token efímero nuevo. Un rechazo del resolvedor cierra la fila del hijo como
`FAILED` y su texto llega verbatim con el prefijo estable (`CHILD_GRAPH_RESOLVE_FAILED:<code>: …`,
vía `Unavailable`, ya lo hacía el ejecutor para inline/path desde la entrada 78);
un agente editado sin cambiar su forma se reanuda con la versión nueva, con otra
forma falla con `SUBGRAPH_RESUME_INCOMPATIBLE:`. Costo nuevo: un resolve por
resume, hasta 30 s (el mismo timeout que el camino fresco). Ningún frame SSE
nuevo — la rama de resume del `SubGraphNode` sigue sin boundary propio.

**Tests.** 4 nuevos en `child_graph_ref_tests` (TDD red-first), reemplazando
`a_ref_still_resumes_its_stored_graph`: `a_ref_resume_asks_the_resolver_again_with_the_request_a_fresh_run_made`
(dos resolves, mismo `agent_id`/`context`/sesión/sesión estable/`parent_path`),
`without_a_suspended_child_the_resolver_is_not_asked` (ya pasaba, la fija),
`a_resolver_that_refuses_on_resume_fails_with_its_code_and_the_executor_gets_unavailable`
y `a_ref_resume_puts_the_resolved_graph_in_no_frame_output_or_debug` (el secreto
resuelto no aparece en el output, en un frame ni en `{:?}` del `ResumeGraph`).
`cargo test -p colmena_dag_engine --lib nodes::subgraph`: 58 → 61 passed. `cargo
test -p colmena_dag_engine --lib`: 2828 → 2831 passed (0 failed, 74 ignorados).

**Mutación.** 3, cada una en rojo y revertida a mano: (1) volver a poner
el caso `ref → Stored` → rojos los 3 tests nuevos que dependen del resolvedor (no
el de "sin hijo", que no llama al resolvedor en ningún caso); (2) derivar el grafo
**antes** de `find_child_session_id_for_resume` → rojo, solo,
`without_a_suspended_child_the_resolver_is_not_asked` (con inline/path esta misma
mutación era inobservable en la entrada 78 — un ref hace la derivación una
llamada real al resolvedor, así que acá sí se nota); (3) en `load_child_graph`,
cambiar el mapeo de error de un ref de `e.to_string()` (el `Display` con el
prefijo) a `format!("{e:?}")` (el `Debug`, sin prefijo) → rojo, el de refusal
(y de paso otros 4 del camino fresco que comparten la misma línea, confirmando
que es la misma función la que sirve a los dos caminos).

**E2E.** PR aparte, a continuación de esta entrada (`review_size.py` no daba los dos
juntos: la integración de Postgres con el `StubResolver`, su fixture y el bump
de corpus pesan solas ~370 líneas), integración nueva `#[ignore]`,
`tests/child_graph_ref_resume.rs`: un `ColmenaEngine` real con un `StubResolver`
(`EngineConfig.child_graph_resolver`, el CLI no configura ninguno) y
`ScriptedAdapter` para el LLM, contra Postgres local (`colmena_e2e_cgr`).
`tests/graphs/agents/child_graph_ref_resume.json` (nuevo; `EXPECTED_FILES` 327 →
328) es el padre — el hijo lo da el stub, no está commiteado. Dos escenarios
reales, capturados en `/tmp/colmena_e2e/`: `child_graph_ref_resume_v2.sse` (el
resolvedor contesta `v1` al arrancar y `v2` al reanudar) — `subgraph-node-end`
de `sello` da `{"sello":"SELLO=v2"}`
(`jq -c 'select(.type=="subgraph-node-end" and .node_id=="sello") | .output'`),
ningún frame contiene el marcador `sk-stub-graph-marker` del grafo completo, dos
resolves con el mismo `agent_id`/`context`/`parent_path`, la fila hija
`COMPLETED`; `child_graph_ref_resume_forbidden.sse` (el segundo resolve rechaza
`Forbidden`) — el parent sigue (no `SUSPENDED`), ningún `sello` corrió, la fila
hija `FAILED`. El texto `CHILD_GRAPH_RESOLVE_FAILED:forbidden: …` no llega por
ningún frame SSE — un resume nunca dispara `LlmToolCallStart`/`Finish` (esos
frames son del camino de despacho fresco de `llm.rs`; el resume llama
`execute_with_resume_answer` y persiste el resultado directo en
`llm_node_history` como mensaje `tool`, que es donde el test lo verifica;
hallazgo de esta task, no anticipado por el plan). Mutación de integración: con
el caso `ref → Stored` reintroducido, los dos escenarios fallan — el primero
corre `SELLO=v1` en vez de `v2` (nunca deriva de nuevo) y el segundo no falla
(el resolvedor rechazado nunca se consulta, el hijo completa con la copia
vieja). Lint: 328 files, 0/0/0. `corpus_noise`: 3 passed. `cargo test` completo
(workspace): 3051 passed, 0 failed, 146 ignorados.

**ADP.** [Nota de migración](adp_migration/2026-09-24-subgraph-resume-fresh-graph.md),
sección «Desde la entrada 81». `docs/adp_migration/2026-09-23-child-graph-ref.md`
actualizada (ya no dice que un resume no vuelve a llamar al resolvedor). Guía 19
(«Grafo por referencia», «Reanudar con el grafo actual», «Resume con árbol de
runs») y `docs/qa/nodes/subgraph.md` (hallazgo cerrado) pierden sus tres
menciones de «hasta un PR posterior»; `docs/developer_guide/30_database_schema.md`
y `docs/node_as_tools_reference.json` igual.

## 82. Una fila de `dag_runs` guarda solo el esqueleto del grafo

Paso 1 de 2 del diseño de secretos en reposo (Startti/adp,
`docs/superpowers/specs/2026-09-25-secretos-en-reposo-dag-runs-design.md`, D1, D3 y D6).
**Comportamiento observable y rompe la API de Rust.**

**Qué cambió.**
- **La forma en reposo.** `GraphSkeleton::at_rest_json(&Graph) -> Value` (en
  `domain/graph_skeleton.rs`) da lo único que una fila guarda del grafo:
  `{"nodes": {id: {"type"}}, "edges": [{"from", "to", "cyclic"?}]}`. `cyclic` va solo
  cuando es `true`. No van `config` (con las claves de proveedor ya resueltas),
  `timezone`, `location`, `locale`, `trigger_on` ni los topes de llamadas.
- **Los seis escritores la usan.** Antes todos hacían `serde_json::to_value(&graph)`:
  - el arranque de un hijo (`run_subgraph`);
  - las dos cancelaciones;
  - el watchdog;
  - el suspend;
  - el fin.

  Vale para la raíz y para el hijo. Desde la entrada 78, el resume de un hijo solo lee
  el esqueleto guardado, y la raíz nunca leyó su `graph_json`.
- **El puerto pierde `ResumeGraph::Stored`** y salen la válvula
  `COLMENA_SUBGRAPH_RESUME_GRAPH=stored`, `stored_resume_valve` y `valve_is_stored`.
  Ya no hay un grafo guardado que se pueda correr. **Rompe al compilar** a un
  embebedor que construya `Stored`.
- **`plan_resume` parsea el guardado solo en `Fresh`**, con el mismo error de antes si
  no se lee.
- **Un par de fixtures fija la forma**: `src/libs/colmena/tests/fixtures/at_rest/graph.json`
  y `graph.at_rest.json`. ADP copia los dos para su backfill.

**Tests.**
- 3 nuevos en `graph_skeleton` (TDD, en rojo primero):
  - igual al fixture;
  - sin nada secreto: ni el centinela, ni `config`, ni el contexto del grafo, ni los
    topes, ni `"cyclic":false`;
  - vuelve a parsear con el mismo esqueleto, sobre el fixture y sobre `base()`.
- `a_row_kept_at_rest_resumes_with_the_fresh_graph` reemplaza a
  `stored_resumes_with_the_graph_the_row_kept`: una fila en reposo se reanuda con el
  fresco (`stamp` v2), y la fila que queda al terminar está en reposo otra vez.
- `an_unparsable_stored_graph_is_todays_error_and_does_not_close_the_row` pasa
  `Fresh`: el guardado se parsea primero también ahí.
- Salen los dos tests de la válvula.
- `cargo test -p colmena_dag_engine --lib`: 2843 passed, 0 failed, 74 ignorados. Son +1 neto contra `develop`: +4 nuevos, −1 reemplazado y −2 de la válvula.

**Mutación.** Cada una por separado, en rojo y revertida:
1. conservar `config` en `at_rest_json` → rojo en el fixture y en «nada secreto»;
2. escribir siempre `"cyclic": e.cyclic` → rojo en el fixture y en «nada secreto»;
3. perder `cyclic: true` → rojo en el fixture y en «vuelve a parsear»;
4. volver a `to_value(&graph)` en el escritor del fin → rojo en
   `a_row_kept_at_rest_resumes_with_the_fresh_graph`.

**E2E.** Determinista, sin LLM, contra Postgres local (`colmena_e2e_rest`, con las
migraciones del crate), sobre `tests/graphs/advanced/subgraph_resume_fresh_graph/`
(corrida 3 de su README, que reemplaza a la de la válvula).
- **Montaje.** Se siembra un centinela `sk-e2e-at-rest-sentinel-…` en la config de
  `fin` del hijo. Turno 1 suspende; turno 2 con `--answer` y `SELLO=v2`.
- **Capturas** en `/tmp/colmena_e2e/graph_at_rest_{pr1,v018,rollback}_{1,2}.sse`.

Columnas del `SELECT` sobre las filas de la raíz y del hijo: estado · centinela en
`graph_json` · `"config"` en `graph_json` · centinela en `global_shared_state` ·
centinela en `all_outputs`.

| Binario | Turno 1 | Turno 2 | `sello` |
|---|---|---|---|
| Esta entrada | `SUSPENDED\|f\|f\|t\|f` | `COMPLETED\|f\|f\|t\|f` | `{"sello":"SELLO=v2"}` |
| Línea base, tag v0.18.0 | `SUSPENDED\|t\|t\|t\|f` | `COMPLETED\|t\|t\|t\|f` | `{"sello":"SELLO=v2"}` |

- `global_shared_state` todavía trae el centinela por `__graph_nodes`. Eso lo cierra
  el paso 2 de esta cadena.
- **Rollback medido:** el turno 1 con este binario (filas sin config) y el turno 2 con
  el de v0.18.0 → `{"sello":"SELLO=v2"}`, las dos filas `COMPLETED`.

Gates:
- `cargo test` completo (workspace): 3052 passed, 0 failed, 146 ignorados;
- `cargo clippy --all-targets -- -D warnings` limpio;
- lint del corpus: 328 archivos, 0/0/0. `check_doc_links`, `check_doc_counts` y `check_hexagonal_documents` en verde.

**ADP.** [Nota de migración](adp_migration/2026-09-25-graph-at-rest.md): no hay cambio
de código, y después del deploy se corre el backfill de las filas viejas. **No fijar la
válvula en v0.18 una vez desplegado v0.19.**
- Guía 19 («Reanudar con el grafo actual»): el párrafo de la válvula pasa a ser el de la
  fila en reposo, con la compatibilidad.
- `docs/developer_guide/30_database_schema.md` (`graph_json`), `docs/qa/nodes/subgraph.md`
  y la nota `2026-09-24-subgraph-resume-fresh-graph.md` actualizados.

## 83. `__graph_nodes` guarda solo las descripciones de los nodos

Paso 2 de 2 del diseño de secretos en reposo (Startti/adp,
`docs/superpowers/specs/2026-09-25-secretos-en-reposo-dag-runs-design.md`, D2). Con la
entrada 82, cierra las claves de la config del grafo en `dag_runs`. **Comportamiento
observable** en el estado guardado.

**Qué cambió.**
- **Antes.** `global_shared_state.__graph_nodes` se armaba en cada entrada con la
  `config` entera de cada nodo: las claves de proveedor, y el `child_graph_inline`
  de un `subgraph` con las claves del hijo. Se persistía con el estado en cada fila,
  y un hijo lo recibía también en la semilla del padre.
- **Ahora.** Se arma con `DagRunUseCase::planner_descriptions(&graph)`:
  `{ "<id>": { "description": "<texto>" } }`, solo para los nodos cuya
  `config.description` es un string.
- **Su único lector, el planner, no cambia.** Lee `__graph_nodes.<id>.description` para
  un agente dado como string, y un nodo sin descripción sigue dando «No description
  provided.».

**Tests.** Los dos en rojo primero:
- `graph_nodes_meta_keeps_only_string_descriptions`: una descripción de texto queda,
  y se descartan un nodo sin descripción, uno con descripción no textual y la
  `api_key`.
- `the_persisted_state_keeps_only_node_descriptions`: el resume de un hijo con la
  config del nodo, centinela incluido, deja en la fila `__graph_nodes` =
  `{"sello": {"description": "Sella"}}` y ningún centinela en el estado.
- `cargo test -p colmena_dag_engine --lib`: 2845 passed, 0 failed, 74 ignorados.

**Mutación.** Guardar `node.config.clone()` en vez de la descripción → rojo en los dos
tests nuevos. Revertida.

**E2E.** La corrida 3 del README de
`tests/graphs/advanced/subgraph_resume_fresh_graph/`, ahora con el `SELECT` de las
tres columnas (captura `/tmp/colmena_e2e/graph_at_rest_pr2_{1,2}.sse`). Las columnas
son: estado · centinela en `graph_json` · `"config"` en `graph_json` · centinela en
`global_shared_state` · centinela en `all_outputs`.
- Esta entrada: turno 1 `SUSPENDED|f|f|f|f` y turno 2 `COMPLETED|f|f|f|f`, en la raíz
  y en el hijo, con `{"sello":"SELLO=v2"}`.
- Con la entrada 82 sola: `global_shared_state` daba `t`.
- Con v0.18.0: `t|t|t|f`.

Gates:
- `cargo test` completo (workspace): 3054 passed, 0 failed, 146 ignorados;
- `cargo clippy --all-targets -- -D warnings` limpio;
- corpus: 328 archivos, 0/0/0;
- `check_doc_counts` y `check_hexagonal_documents` en verde.

**ADP.** [Nota de migración](adp_migration/2026-09-25-graph-at-rest.md), sección
«Qué cambió (entrada 83)». El backfill de ADP (`scrub-dag-runs-at-rest`, Startti/adp#830)
también reduce `__graph_nodes` en las filas viejas. Actualizados: guía 19 («Reanudar
con el grafo actual»), `docs/developer_guide/30_database_schema.md` (`graph_json` y
`global_shared_state`) y el README del E2E. **Tag `colmena_dag_engine-v0.19.0`**
después de esta entrada.

## 84. Fix: un turno detenido ya no vuelve a correr su cola en el turno siguiente

**El bug.** Reportado por ADP (sesión `cmufruoxp000n01s68novaivi`, 2026-09-24): el
usuario detuvo un turno y escribió «procede con el plan», y el LLM raíz recibió el
mensaje anterior. ADP manda en cada turno el `session_id` del run del chat, así que cada
turno entra por la rama 1 de `execute_stream` (resume directo por id), que tomaba
`active_queue` de la fila sin mirar `status`. Un turno cancelado a mitad de un nodo guarda
`CANCELLED` con la cola `[nodo en vuelo, …]` y los outputs del turno, el del nodo de
entrada con el mensaje viejo incluido. El turno siguiente arrancaba por esa cola: el nodo
de entrada no corría y el LLM se armaba con el mensaje viejo de `all_outputs`. Con
`COMPLETED` no pasaba porque la cola queda vacía. La fila `FAILED` que deja el watchdog de
inactividad guarda la cola igual y daba el mismo efecto.

**Qué cambió.** `DagRunUseCase::resumes_from_stored_queue` decide cada `DagRunStatus` sin
comodín. `SUSPENDED` retoma su cola, como antes. `COMPLETED` y `RUNNING` se guardan con la
cola vacía y quedan como estaban. `CANCELLED` y `FAILED` no la retoman: el turno arranca
desde los nodos de entrada, como después de `COMPLETED`, y el resto de la fila (outputs,
estado compartido, historial, contadores) se conserva como antes. Para esas filas tampoco
se calcula `resuming_node_ids`: un `__colmena_status: "SUSPENDED"` que quedó en los
outputs (un turno que respondió una pregunta y se detuvo antes de que su nodo volviera a
correr, p. ej. un Stop durante el pre-flight) ya no recibe el `answer` de un turno
posterior. La rama 2 (`find_resume_entry`) ya filtraba `SUSPENDED`.

**Tests.** 5 en `run_use_case.rs` (`stored_run_status_tests`). Cada estado guardado lo
escribe el motor en un turno anterior; ninguno es una fila armada a mano. En rojo antes
del fix:
- tras un `CANCELLED` (Stop con el LLM en vuelo) el LLM recibía `"old prompt"`;
- tras un `FAILED` (watchdog) también;
- el marcador viejo recibía el `answer` `"otra"`.

En verde antes y después: un `SUSPENDED` retoma su cola con el `answer`, y tras un
`COMPLETED` el turno arranca por la entrada. `cargo test` completo: 3059 passed,
0 failed, 146 ignorados.

**Mutación.** 4, rojas y revertidas:
- tomar la cola sin mirar el estado → los 3 rojos;
- calcular los marcadores sin mirar el estado → el del marcador;
- `SUSPENDED` como detenido → la guarda de `SUSPENDED`;
- `FAILED` como retomable → el de `FAILED`.

**E2E.** `tests/graphs/basic/stopped_turn_fresh_queue.json` por el CLI contra un Postgres
local, en dos turnos con el mismo `--session-id` (el turno 2 se deriva con `jq`, como dice
el `comment` del grafo). El Stop no se puede disparar desde el CLI; el camino `FAILED`
sí, con `COLMENA_IDLE_TIMEOUT_SECS=2`. En los dos binarios el
turno 1 deja la fila `FAILED` con la cola `["modelo"]`. En el turno 2:
- sin el fix (develop `78177dfc`), el SSE empieza en `modelo`, sin nodo de entrada como el
  árbol del reporte, y devuelve `{"vio": "mensaje viejo"}`;
- con el fix, empieza en `entrada` y devuelve `{"vio": "mensaje nuevo"}`; la fila queda
  `COMPLETED` con el mensaje nuevo en `all_outputs.entrada`.

**ADP.** Sin cambio de contrato; basta con subir el motor. Tras un Stop, el turno
siguiente vuelve a emitir los frames del nodo de entrada, como cualquier turno.

**Revisión (3 fixes, mismo release).**

1. El bloqueo de inyección duraba un solo turno: el marcador `__colmena_status:
   "SUSPENDED"` seguía en `all_outputs` aunque su nodo no volviera a correr. En un
   grafo con ruteo, ese nodo podía quedar fuera del turno fresco que sigue a
   `CANCELLED`/`FAILED` (el guardado `COMPLETED` de ese turno conservaba el
   marcador), y un turno posterior que suspendiera en un nodo distinto lo
   arrastraba como si también estuviera esperando esa respuesta, inyectándosela
   si el nodo llegaba a correr. `DagRunUseCase::drop_stale_suspended_outputs`
   ahora borra del `all_outputs` cargado, al leer una fila no retomada, toda
   entrada que siga marcada `SUSPENDED` — no solo bloquea la inyección de ese
   turno, la elimina para que no reaparezca. Test:
   `a_stale_suspended_marker_dropped_at_load_never_resurfaces_later` (grafo de 5
   turnos: suspende en `llm_a`, se cancela antes de retomar, un turno enrutado a
   `llm_x` nunca toca `llm_a` y completa, `llm_b` suspende con su propia
   pregunta, y al responderla `llm_a` vuelve a correr río abajo de `llm_b` sin
   recibir esa respuesta). En rojo antes del fix: `llm_a` recibía
   `Some("respuesta")`; mutación (comentar la llamada) confirma que solo ese
   test cae.
2. Una fila `SUSPENDED` hija de una raíz que termina `CANCELLED`/`FAILED` quedaba
   huérfana: `cancel_running_descendants` solo cierra descendientes `RUNNING`. La
   raíz nunca vuelve a retomar a esa hija, pero la fila seguía abierta y
   `find_resume_entry`/`find_suspended_child` podían recogerla más tarde (ver la
   sección "Concerns" del reporte de este fix). Los 3 puntos de guardado
   terminal de `execute_stream` (entre nodos, a mitad de nodo, watchdog de
   inactividad) ahora llaman también a `repo.fail_suspended_descendants` —ya
   existía, usado por `close_refused` en el rechazo de un resume— junto al
   `cancel_running_descendants` existente. 3 tests, uno por sitio
   (`a_root_cancelled_between_nodes_closes_its_suspended_child`,
   `a_root_cancelled_mid_node_closes_its_suspended_child`,
   `a_root_failed_by_the_idle_watchdog_closes_its_suspended_child`), cada uno
   rojo solo cuando se quita su propio sitio (mutación: eliminar cada llamada
   por separado, más las 3 juntas).
3. Nota para ADP: un turno detenido que ya persistió el mensaje `user` deja esa
   fila colgando; el turno siguiente vuelve a mandar el suyo. `LlmRequest::new`
   (`llm_request.rs`, `coalesce_consecutive_same_role`) fusiona mensajes
   consecutivos del mismo rol antes de armar el request — así que el modelo ve
   «mensaje viejo\n\nmensaje nuevo» en un solo turno `user`. Comportamiento
   previo a este fix, documentado aquí porque es la superficie donde un stop se
   nota si no se lo espera; no es un bug nuevo.

4 tests nuevos (1 del punto 1, 3 del punto 2) sobre los 3059 que dejó el fix
original: 9/9 en `stored_run_status_tests`, estable en 20 corridas seguidas.
`cargo test` completo (`--verbose`): 3063 passed, 0 failed, 146 ignorados.

## 85. Fix: `$attachment:<document_id>` en un body JSON pasa por el registro de la sesión

**El bug.** El camino JSON de `http_request` leía el id de `"$attachment:<id>"` como clave
de storage. Desde el Plan A el catálogo enseña `document_id`s, así que ningún adaptador
resolvía un id del catálogo por JSON (ADP: `body.image_url = "$attachment:<document_id>"` →
`not found in HttpCallback meta cache`), y la guía 32 decía lo contrario. Además, el JSON y
el respaldo del resolver de multipart leían cualquier clave cruda escrita por el modelo, de
esta sesión o de otra.

**Qué cambió.**
- JSON: con resolver cableado, el placeholder se resuelve con el `AttachmentStreamResolver`
  de la sesión, como multipart, y se vuelve `data:<mime>;base64,…` (tope
  `max_file_size_bytes`). Sin registro en el motor, sigue leyendo la clave (legacy).
- `AttachmentStreamResolverImpl` ya no lee el id como clave cruda: lo que no es un
  `document_id` de la sesión es `NotFound`, sin tocar el storage.
- `DagToolExecutor::register_attachment_bytes` (`data_run_python`, `gdocs_export`,
  `gsheets_export_xlsx`) registra su fila en la sesión, con `document_id` = la clave que ya
  devolvía; si no, cerrar las claves crudas rompía reenviar esas salidas. Efecto lateral:
  aparecen en el catálogo del turno siguiente y el `attachment_gc` las borra por TTL.
- Preludio de adjuntos, guías 25 y 32 y `node_as_tools_reference.json`: JSON → `data:` URI
  (útil solo si la API acepta data URIs); multipart → parte de archivo; clave cruda →
  rechazo.

**Tests.** `http.rs::session_attachment_tests` (4, resolver real sobre SQLite),
`stream_resolver_impl` y `dag_tool_executor` (uno cada uno); rojos antes del fix.
**ADP.** Una clave cruda en `$attachment:` da ahora `attachment not found`: va el
`document_id` del catálogo. **Otros hosts del motor.** Todo `ColmenaEngine` cablea un
registro (`engine.rs`), así que un run sin `agent_session_id` falla `$attachment:` en un
body JSON («needs an agent_session_id») donde antes leía la clave cruda, y no registra sus
exportaciones. Los bindings de Python y Node no cablean registro: sin cambios.
**Estado.** done (punto 11, parte A; sigue `image_edit`).

**Revisión (mismo release).** Las exportaciones que registra `register_attachment_bytes`
llevan `origin: generated_by:<tool>`, que el catálogo muestra. El `NotFound` agrega «use a
document_id from the attachments catalog». `HttpCallbackStorageAdapter` saca la URL de los
errores de reqwest (`without_url()`): una URL firmada de lectura o de subida ya no llega al
error que ve el modelo (el camino JSON ahora pasa por `read_stream`). Tests: los de
`register_attachment_bytes` y del rechazo JSON de `http.rs`, extendidos, y
`a_transport_error_never_carries_the_signed_url`; rojos antes del fix, mutaciones muertas.

## 86. Una entrada de tool puede declarar `parallel`, y solo como booleano

**Qué cambió.** Una entrada de `tool_configurations` acepta `"parallel": true`: opt-in,
por entrada, para que cada llamada de esa tool tenga su propia identidad en el stream.
En este cambio solo se acepta y se valida; la corrida todavía no hace nada con él:
- `Graph::validate` rechaza al cargar un `parallel` que no es booleano, con un error que
  nombra el campo y la tool;
- `dag_engine lint` lo espeja como `MALFORMED_TOOL_ENTRY` (quinta compuerta de
  `other_validate_rejections`), así que el linter sigue cubriendo todas las de `validate`;
- `ToolConfiguration.parallel` (por defecto `false`, no se serializa cuando es `false`) y
  `ToolCall.scope_index` (`#[serde(skip)]`, `None` en todos los sitios que arman un
  `ToolCall`) quedan listos para los cambios que los usan.

Frames, memoria y orden de las llamadas: iguales que antes.

**Tests.** 3 en `graph.rs` (rechaza `"yes"`, acepta `true`, acepta la clave ausente) y 1
en `tests/graph_lint.rs`, más `{ "parallel": true }` en
`entries_the_engine_accepts_are_left_alone`. La lib: 2857 passed (2854 antes);
`graph_lint`: 101.

**Mutación.** 3, rojas y revertidas: la polaridad del chequeo de `validate` invertida
(caen el que rechaza y el que acepta `true`), el error de `validate` sin nombrar campo ni
tool, y la compuerta del linter apagada (`a_non_boolean_parallel_is_reported`).

**E2E.** Por el CLI, con el grafo de dos tools `subgraph` (`Run` con `parallel`): con
`"parallel": true` el lint da 0/0/0; con `"parallel": "yes"` el lint da
`MALFORMED_TOOL_ENTRY` en `tool_configurations.Run.parallel` y `dag_engine run` sale con
código 1 y `Invalid graph: ... 'parallel' must be a boolean, got: "yes"` antes de correr
nada. Corpus: 329 archivos, 0/0/0.

**ADP.** Nada que cambiar. Un `parallel` no booleano ahora se rechaza al cargar; antes
se ignoraba. Documentado en `docs/node_configurations.json`,
`docs/node_as_tools_reference.json` y en la lista de compuertas de `Graph::validate()` de
las guías 48, 49 y 51.

## 87. Una llamada a una tool `parallel` abre su frontera como `<tool>#<k>`

**Qué cambió.**
- `agent_service` le pone a cada llamada su k (`ToolCall.scope_index`, su índice en el
  mensaje `tool_calls` del modelo) antes de despacharla, también a la que contesta el guard
  de repetición.
- Con streaming, las llamadas se ordenan por el índice del proveedor antes de persistirlas y
  despacharlas; antes salían en el orden de un `HashMap`. Vale para toda tool.
- `ToolExecutor::child_scope`, `None` por defecto. En `DagToolExecutor` es `<tool>#<k>`
  cuando la entrada tiene `"parallel": true` y la llamada trae k. Encuentra la entrada con
  `configured_tool` (clave del mapa y después `name`), el mismo helper que ahora usa el paso
  1 del despacho.
- Ese nombre va a `__colmena_tool_name` (la frontera de un `subgraph`) y a la frontera y al
  `ChildScopeObserver` que el ejecutor abre para `llm_call` y `for_each`. La memoria sigue en
  `tool/<tool>/<thread>`.

Los frames de la tool todavía no dicen qué frontera abrió cada llamada. Una tool sin
`parallel` abre la frontera con el nombre pelado, como antes. Las llamadas siguen en serie.

**Tests.** 9 en la lib, 2866 passed (2857 antes):
- 7 en `dag_tool_executor::parallel_identity_tests`: el scope con y sin opt-in y sin k, la
  entrada encontrada por `name`, la frontera de un `subgraph` con la memoria intacta, y un
  `llm_call` como tool. Su nodo de prueba emite un token, y el test exige que llegue sellado
  `Helper#3` entre la apertura y el cierre de la frontera.
- 2 en `agent_service`: seis llamadas que llegan por stream en orden `[1,0,5,3,2,4]` corren
  y se persisten en orden de índice, cada una con su k; la que contesta el guard de
  repetición también trae su k.

**Mutación.** 7, rojas y revertidas: sin el filtro de `parallel`; la frontera del
`llm_call` con el nombre pelado; sin su `ChildScopeObserver` (el token no llega) o con él
bajo el nombre pelado (llega sellado `Helper`); la memoria keyada por el scope; sin ordenar
por índice (corren `c2, c4, c3, c5, c1, c0`); y `agent_service` sin k (caen los 2).

**E2E.** Corrida ad hoc, sin commitear, de un `ColmenaEngine` real contra Postgres con el
grafo de dos tools `subgraph` y un modelo guionado que pide `Run`, `Nota` y `Run` en un solo
mensaje (`Run` con `parallel`): las fronteras salen `agent>Run#0`, `agent>Nota` y
`agent>Run#2`, en serie, y ningún frame trae `childScope`.

**ADP.** Nada que hacer: ADP todavía no emite `parallel`, y no debe emitirlo hasta que los
frames de la tool nombren su frontera. Actualizados `docs/node_configurations.json` y
`docs/node_as_tools_reference.json`.
## 88. Fix: `image_edit` resuelve `$attachment:<document_id>` y deja de leer handles del modelo

**El bug.** `image_edit.source_url` solo aceptaba `data:`, `http(s)` y los handles
`local://…`/`chat-attachments/…`, que leía con `storage.read`. No resolvía
`$attachment:<document_id>` ni un `document_id` pelado, aunque `image_generation` e
`image_edit` le decían al modelo que los usara «in downstream tool args» (lo que hizo #76
fase 1 y deshizo #79). Y un handle escrito por el modelo se leía tal cual: cualquier clave
que el storage sirviera.

**Qué cambió.** Con registro de adjuntos, `source_url` y `mask_url`: `data:`/`http(s)` se
usan tal cual; cualquier otra cosa es `"$attachment:<document_id>"` (o el id pelado) de la
sesión, resuelto con `AttachmentStreamResolverImpl` sobre el registro y el storage del nodo
(tope 100 MiB); un handle o una clave cruda da `attachment not found` sin leer nada. Sin
registro (motor standalone), los handles se siguen leyendo (legacy). Las descripciones de
`image_generation`, `image_edit` y `tts` dicen qué produce cada forma. Guías 31 y 32,
`node_as_tools_reference.json` y `node_configurations.json`, al día; la «Limitación
conocida» de la guía 32 se borró.

**Tests.** 2 en `image_edit.rs` (fuente y máscara por `document_id`; handles de ADP,
`local://` y claves crudas → error). Rojos antes del fix. **ADP.** El bloque
`<session-images>` enseña handles `chat-attachments/…` para `image_edit.source_url`: con
este cambio fallan (ya fallaban entre procesos desde #79); tiene que pasar `document_id`s.
**Estado.** done (punto 11, parte A, 2/2).

## 89. Fix: un edge ya no elige la sesión cuyos adjuntos lee un nodo

**El bug (preexistente).** En modo grafo, el loop inyecta `__colmena_agent_session_id` solo
cuando el run tiene sesión de agente, y `build_inputs_for` aplana en los inputs las claves
del objeto que llega por un edge sin puerto a un nodo sin `default_input` (`http_request`,
`python_script`). Nada descartaba las claves del motor que traía ese objeto. En un run con
registro de adjuntos y sin sesión, un `trigger_webhook` o un LLM conectado así a
`http_request` con
`{"__colmena_agent_session_id":"<víctima>","body":{"f":"$attachment:<doc de la víctima>"}}`
resolvía el documento de otra sesión y lo mandaba. Con sesión, el motor pisaba esa clave,
pero no `__colmena_resume_answer`, que fuera de un resume no inyecta: un payload podía
retomar un hijo suspendido de un `subgraph` con una respuesta propia (`suspend` y
`secure_suspend` tienen `default_input`, así que a ellos no llegaba).

**Fix.** `strip_engine_keys` pasa al dominio (`dag_engine/domain/node.rs`; la usan el despacho
de tools y `for_each` como antes) y `build_inputs_for` la aplica a los inputs que arma: toda
clave `__colmena*`/`__node*` que trae un edge se descarta, y el loop escribe después las suyas.
Lo que llega por el estado global (`__colmena_subgraph_depth` en un hijo) no cambia: se inyecta
después.

**Tests.** En `http.rs::session_attachment_tests`, un grafo real `trigger_webhook` →
`http_request` con el resolver sobre SQLite: sin sesión, el payload falsificado da `needs an
agent_session_id` y el servidor no recibe nada (rojo antes del fix: recibía el documento de
`s2`); con sesión, `attachment not found`; y el id de sesión del motor sigue llegando (resuelve
el documento propio). Mutaciones: sin el strip cae el primero; con un strip también después de
la inyección caen los dos; el dominio sin `__node` tumba los 3 tests de strip de tools y
`for_each`.

**E2E.** `tests/graphs/security/graph_edge_engine_keys_e2e.json` (`python_script` como
testigo; `EXPECTED_FILES` 329 → 330). Antes del fix: sin sesión veía `forged-session`, y con o
sin sesión `forged-answer`. Después: sin sesión `<ABSENT>`, con `--agent-session-id e2e_real`
el id real; `__colmena_resume_answer` `<ABSENT>`; `__node_id` = `witness`; `probe` llega. Con
registro Postgres y storage LocalHttp, el mismo payload hacia `http_request`: antes del fix
mandaba al eco los bytes de la otra sesión; después, `needs an agent_session_id` sin sesión y
`attachment not found` con sesión, y el eco no recibe nada; con la sesión propia resuelve su
documento. **ADP.** Sin cambios de API; un grafo que mandaba a propósito una clave
`__colmena*`/`__node*` por un edge deja de recibirla (ninguno en `tests/graphs`).
**Estado.** done (punto 11, parte A, revisión).

## 90. Los frames de una llamada a una tool `parallel` nombran su frontera (`childScope`)

**Qué cambió.** Cada llamada de una tool con `"parallel": true` ya abría su frontera como
`<tool>#<k>` (entrada 87). Ahora sus frames lo dicen:
- `NodeEvent` y `DagExecutionEvent::LlmToolCallStart/Finish` suman `child_scope`, opcional
  y omitido cuando no hay valor. `from_node_event` y el mapeo de `run_use_case.rs` lo pasan.
- En `llm.rs`, el callback del stream le pide el scope al ejecutor en el Start
  (`ToolExecutor::child_scope`) y se lo da al Finish de la misma llamada por su id
  (`ToolCallScopes`).
- El `SseMapper` agrega `childScope` a `tool-input-available`, `tool-output-available` y
  sus variantes `subgraph-tool-*`. Sin valor, el frame queda igual byte a byte.
- Un resume vuelve a correr la llamada pendiente con el k que tenía en su mensaje
  (`find_pending_tool_call` devuelve también su índice), así que el hijo reabre bajo el
  mismo `<tool>#<k>`.

`tool-input-start` no lleva `childScope`: sale del chunk del stream, antes de que la
llamada tenga su k. El `childScope` de una llamada se lee de su `tool-input-available`.

**Tests.** 8 en la lib, 2874 passed (2866 antes): 4 en `sse_mapper` (con scope, anidado,
sin scope igual al frame de antes campo por campo, y el evento que serializa el campo solo
con valor) y 4 en `llm.rs` (el Finish recibe el scope de su Start, dos llamadas abiertas
guardan cada una el suyo, una llamada sin scope cierra sin él, y el índice de la llamada
pendiente en su mensaje llega al resume).

**Mutación.** 4, rojas y revertidas: `close` que pierde el scope (caen 2), el mapper que
descarta `childScope` (caen los 2 que lo esperan), el mapper que escribe `null` cuando no
hay scope (cae el del frame igual al de antes) y el resume sin k.

**E2E.** Corrida ad hoc, sin commitear, del mismo E2E que en la entrada 87 (`ColmenaEngine`
real contra Postgres, `Run`, `Nota` y `Run` en un mensaje): pasa entero. 45 frames:
`childScope` `Run#0` y `Run#2` en `tool-input-available` y `tool-output-available` de cada
`Run`, cada frontera entre los dos frames de su llamada; ninguno en los de `Nota` ni en los
tres `tool-input-start`.

**ADP.** Soportar `childScope` antes de subir el pin; ya está en Startti/adp#855. Los frames
de una tool sin `parallel` no cambian. Actualizados `docs/sse_events_reference.md` (tablas y
la sección «`childScope` — una llamada a una tool `parallel`»),
`docs/node_configurations.json` y `docs/node_as_tools_reference.json`.

## 91. E2E de la identidad de una llamada `parallel`, y la nota para ADP

**Qué cambió.** Nada en el motor. Llegan el E2E que prueba de punta a punta las
entradas 87 y 90, la sección de la guía 19 y la nota de migración para ADP:
- `tests/graphs/agents/parallel_tool_identity.json`: un `llm_call` con `stream: true` y dos
  tools `subgraph` con un hijo inline trivial (`entrada → salida`). `Run` declara
  `parallel`; `Nota`, no.
- `src/libs/colmena/tests/parallel_tool_identity.rs`, `#[ignore]` y `#[serial]`: un
  `ColmenaEngine` real contra Postgres. `ScriptedAdapter` da una sola tool call por
  respuesta, así que el test trae su propio modelo guionado (`ParallelTurnModel`, por
  `OverrideGuard`). Ese modelo pide `Run`, `Nota` y `Run` en un solo mensaje (tres chunks
  con índices 0, 1 y 2) y después contesta «Listo.».
- `corpus_noise`: `EXPECTED_FILES` pasa de 330 a 331.

**Tests.** Ninguno nuevo en la lib (2874 passed). `cargo test` completo: 3084 passed, 0
failed, 147 ignorados (146 antes, más este E2E).

**Mutación.** 4 contra el E2E, rojas y revertidas: k fijo en 0 (`agent>Run#0` dos veces);
sin el filtro de `parallel` (aparece `agent>Nota#1`); el mapper que escribe `childScopeX`; y
el callback del stream de `llm.rs` que no le pasa el scope al Start (`childScope` ausente
en `tool-input-available`). Esa última unión solo la cubre este E2E.

**E2E.** `DATABASE_URL=postgres:///colmena_e2e_par SECURE_VALUES_KEY=... cargo test -p
colmena_dag_engine --test parallel_tool_identity -- --ignored`: 1 passed. La base tiene las
14 migraciones. El SSE queda en `/tmp/colmena_e2e/parallel_tool_identity.sse`, 45 frames, y
se parsea frame por frame:
- las fronteras son `agent>Run#0`, `agent>Nota` y `agent>Run#2`;
- cada `Run` trae `childScope` `Run#0` o `Run#2` en `tool-input-available` y en
  `tool-output-available`, y su frontera queda entre los dos frames;
- los frames de `Nota` no traen la clave;
- hay tres `tool-input-start`, ninguno con `childScope`;
- corre en serie: cada `tool-input-available` llega después del `tool-output-available`
  anterior.

Corpus: 331 archivos, 0/0/0.

**ADP.** [Nota de migración](adp_migration/2026-09-25-parallel-tool-calls.md), con su
línea en el índice: soportar `childScope` antes de subir el pin (hecho en Startti/adp#855)
y leerlo de `tool-input-available`. La guía 19 suma «Varias llamadas a la misma tool en un
turno (`parallel`)». La referencia SSE cita este E2E, y `node_as_tools_reference.json` lo
da como ejemplo verificado.
## 92. `OutputStorageRepository` gana un método opcional `read_url` (feature C, parte 1 de 2)

**Qué cambió.** El puerto `OutputStorageRepository` (`output_storage_repository.rs`) gana
dos métodos, ambos con cuerpo por defecto — **aditivo**: un host que implementaba el
trait antes de este cambio compila y corre sin tocar una línea:

- `async fn read_url(&self, storage_key: &str, ttl_seconds: u64) -> Result<Option<String>,
  StorageError>` — pide al host una URL de lectura para una clave ya existente, con un
  TTL sugerido. Default `Ok(None)` ("este host no ofrece URLs de lectura"). Distinto del
  campo `read_url` que ya devuelve `store()`: aquél se emite al escribir un objeto nuevo;
  este método se pide después, para un objeto que puede ser viejo.
- `fn supports_read_url(&self) -> bool` — pista de capacidad, default `false`, en sync
  (no async — `#[async_trait]` deja intactos los métodos que no son `async fn`, como ya
  hace `SkillRepository::list_available`). La usará la parte 2 (el placeholder
  `$attachment_url:` en `http_request`, todavía sin tocar) para no enseñarle al modelo
  una forma que en este host va a fallar siempre.

**La firma sigue sin vivir en la librería.** Ninguno de los dos métodos nuevos llama a
ningún protocolo de firmado; el default es puro y el único adaptador que sobreescribe
`read_url` (`LocalHttpStorageAdapter`) solo reconstruye la URL de su propio servidor
`axum` local — no firma nada. Un host que quiera URLs reales (ADP, firmando un GET de
GCS) implementa su propio adaptador, como ya hace con `ChildGraphResolverPort` (#317,
#806).

**Comportamiento por adaptador** (los tres que ya existían):

| Adaptador | `read_url` | `supports_read_url` |
|---|---|---|
| `LocalCacheStorageAdapter` | default (`Ok(None)`) — no lo sobreescribe | default (`false`) |
| `LocalHttpStorageAdapter` | reconstruye `http://127.0.0.1:<port>/files/<key>` tras el mismo chequeo de path-traversal + existencia que `read()`; `ttl_seconds` se acepta pero no tiene efecto (el servidor estático no expira) | `true` |
| `HttpCallbackStorageAdapter` | default (`Ok(None)`) — no lo sobreescribe; ADP implementará su propio adaptador en el worker para esto | default (`false`) |

**Tests.** 3 en `output_storage_repository.rs` (default `Ok(None)` para un host que no lo
implementa, ejercitado como `Arc<dyn OutputStorageRepository>` real y no solo como
chequeo de compilación; el hint de capacidad en `false`; el argumento `ttl_seconds` llega
sin modificar a una implementación que lo sobreescribe, vía un adaptador espía). 5 en
`local_http_adapter.rs` (URL que efectivamente resuelve un GET con los bytes correctos;
clave inexistente → `InvalidInput`; path-traversal rechazado igual que `read()`; el mismo
storage_key da la misma URL con un TTL de 1s y de 24h — no hay expiración real que
verificar; `supports_read_url() == true`). 2 en `local_cache_adapter.rs` y 2 en
`http_callback_adapter.rs` (default `None`/`false` incluso para una clave que sí existe;
para `HttpCallbackStorageAdapter`, contra un puerto que rechaza la conexión, para probar
que el default no intenta ninguna llamada de red). 12 tests nuevos en total. Lib completa
(`cargo test -p colmena_dag_engine`, todas las suites): 2887 passed, 0 failed, 74 ignored
en la unitaria (2875 passed antes de este PR — 74 ignored sin cambio), más integración y
doctests en verde sin ningún `FAILED` en ninguna suite.

**Mutación.** 3, rojas y revertidas: (1) invertir el default del puerto a `Ok(Some(...))`
tumba `default_read_url_is_ok_none_for_a_host_that_does_not_implement_it`; (2) quitar el
chequeo de existencia en `LocalHttpStorageAdapter::read_url` (devolver la URL sin
comprobar el archivo) tumba `read_url_unknown_key_errors`; (3) ignorar `ttl_seconds` en el
adaptador espía de `output_storage_repository.rs` (no guardarlo) tumba
`ttl_argument_reaches_the_implementation`.

**E2E.** No aplica — este PR es solo el puerto y su documentación; ningún nodo ni el motor
del grafo llaman a `read_url`/`supports_read_url` todavía (eso es la parte 2, el
placeholder `$attachment_url:` en `http_request`, explícitamente fuera de alcance aquí).
No hay comportamiento observable por un grafo real que verificar en este PR; se verifica
en la parte 2, cuando el placeholder exista.

**ADP.** Sin cambios de API para lo que ya usa — `read`/`read_stream`/`store`/`delete` no
cambian de firma. Nada que actualizar hoy; cuando el worker de ADP quiera URLs reales
firmadas, implementa `read_url`/`supports_read_url` en su propio adaptador
`OutputStorageRepository` (no antes de la parte 2, que es la que efectivamente los usa).
**Estado.** partial (feature C, parte 1 de 2 — falta la parte 2: el placeholder
`$attachment_url:` en `http_request`, la sustitución fuera del contexto del LLM, y el
adaptador de ADP).

## 93. El merge de una llamada a una tool devuelve sus avisos (parallel tools, 2a)

**Qué cambió.** `DagToolExecutor::execute_inner` resolvía la entrada y su nodo, parseaba
los argumentos del modelo y los mergeaba en la config del autor, todo en línea. Esos
pasos 1-3 pasan, sin cambios, a `merge_call -> MergedCall`, y la resolución del
`thread_id` pasa a `thread_of`. Es la preparación de la clave de cadena del paso
siguiente, que mergea cada llamada `parallel` igual que su despacho, antes de
despacharla.
- Fix: el merge ya no imprime sus dos avisos (una fuente de grafo hijo que la tool no
  ofrece, un argumento que choca con un campo fijo). `merge_call` los devuelve en
  `MergedCall.warnings` y `execute_inner` los imprime una vez. Si no, una llamada
  mergeada dos veces avisaría dos veces.
- `drop_unoffered_child_graph_sources` y `merge_args_into_schema` conservan su nombre y
  siguen imprimiendo, para `for_each` y los tests; las variantes `*_silently` devuelven
  los avisos.
- Un cambio chico: los avisos salen después de un merge exitoso. Un merge que falla
  después (un node_schema inválido) ya no imprime los argumentos ignorados; la llamada
  falla igual, con su error.

**Tests.** 1 nuevo, `merging_a_call_returns_its_warnings_instead_of_printing_them`: una
llamada a `Run` con `thread_id` (choca con el `${agentId}` fijo) y `child_graph_inline`
(no ofrecido) devuelve los dos avisos, en ese orden. El traslado a `merge_call` lo
cubren los tests del despacho que ya existían, sin tocarlos (por ejemplo
`an_unresolved_fixed_thread_id_is_an_error_not_a_shared_thread`). Lib: 2896 passed,
0 failed, 74 ignored (2895 antes).

**Mutación.** `MergedCall { warnings: Vec::new(), .. }` en `merge_call`: el test nuevo
en rojo (`left: 0, right: 2`). Revertida editando; verde.

**E2E.** No aplica: fuera de cuántas veces sale un aviso por stderr, un grafo no ve
ninguna diferencia.

**ADP.** Nada que hacer.

## 94. La clave de cadena de una llamada `parallel` y `plan_batches` (parallel tools, 2b)

**Qué cambió.** Las dos piezas puras con las que el loop del agente va a armar las
tandas. Todavía nada las usa: el loop sigue corriendo las llamadas de a una.
- `ToolExecutor::parallel_chain_key`, que por defecto da `None`. `DagToolExecutor` le da
  una clave a cada llamada de una tool `parallel` según su hilo de memoria: el nombre de
  la tool y el hilo resuelto en `dynamic`, el nombre en `persistent`, el id de la llamada
  en `stateless`. Resuelve el hilo con `resolved_thread`, que usa el mismo `merge_call` y
  el mismo `thread_of` que el despacho (entrada 93), así que la clave es el hilo en el
  que corre la llamada. Una llamada `dynamic` sin hilo usable queda con el nombre de la
  tool: falla antes de llegar a la memoria. Es la clave revisada junto con las entradas
  86, 87, 90 y 91, y guardada hasta que algo la usara.
- `plan_batches` (`llm/application/tool_batches.rs`), una función pura. En el orden del
  modelo, una llamada sin clave corre sola, como barrera, y las llamadas seguidas con
  clave forman un grupo, con una cadena por clave (misma clave, misma cadena, en orden).

**Tests.** 9 nuevos. Lib: 2905 passed, 0 failed, 74 ignored (2896 en la entrada 93).
- 5 de la clave: dos llamadas al mismo agente comparten clave, a agentes distintos no,
  `persistent` encadena por nombre, `stateless` deja cada llamada sola, y una tool sin
  `parallel` no tiene clave;
- 4 de `plan_batches`: la barrera antes y después de un grupo, la clave repetida que
  alarga su cadena, la entrada vacía y todas sin clave.

**Mutación.** Rojas y revertidas editando:
- la clave `dynamic` sin el hilo (solo el nombre): rojos
  `two_calls_to_the_same_agent_share_a_chain_key` (`Some("Run")` contra
  `Some("Run\u{1f}agent-a")`) y `calls_to_different_agents_have_different_chain_keys`;
- la clave repetida que abre otra cadena: rojo
  `repeated_key_extends_its_chain_in_model_order` (`[[0], [1], [2]]` contra
  `[[0, 1], [2]]`).

**E2E.** No aplica: ninguna de las dos piezas corre todavía en un grafo.

**ADP.** Nada que hacer.

## 95. El guard de repetición, el registro y la suspensión pasan a métodos (parallel tools, 2c)

**Qué cambió.** Un refactor de `AgentService::run` sin cambio de comportamiento. Prepara
el grupo concurrente, que va a usar estas piezas desde otro camino; por ahora solo las
llama el loop en serie. Los comentarios viajan con el código.
- La racha del guard (las tres variables `streak_*`) pasa a `RepeatStreak { sig, count,
  first }`, con `advance(&ToolCall) -> u32`. Misma regla: cuenta las repeticiones
  seguidas de una firma y se reinicia cuando aparece otra.
- `answer_repeat`: contesta una repetición con el aviso en vez de correrla (sus frames
  Start y Finish, su entrada en las llamadas ejecutadas, su mensaje `tool`).
- `record_result`: la entrada de un resultado en las llamadas ejecutadas y su mensaje
  `tool`.
- `suspend`: cierra con el marcador «NO se ejecutó» las llamadas sin resultado y
  devuelve `LlmResponse::suspended`.
- Un detalle de orden: la entrada en las llamadas ejecutadas se agrega después del
  frame Finish, no antes. Nada lee esa lista entre los dos.

**Tests.** Ninguno nuevo ni tocado. Los 40 de `agent_service` pasan igual, entre ellos
`two_identical_calls_in_one_turn_nudges_the_second`,
`suspend_closes_tool_calls_left_unexecuted_in_the_same_batch` y los de
`LOAD_ATTACHMENT`. Lib: 2905 passed, 0 failed, 74 ignored, como en la entrada 94.

**Mutación.** Sobre el código movido, para ver que los tests de siempre lo cubren. Rojas
y revertidas editando:
- `suspend` sin cerrar ningún id (`.take(0)`): rojos
  `suspend_closes_tool_calls_left_unexecuted_in_the_same_batch` y
  `suspend_preserves_results_of_calls_that_ran_before_it`;
- `RepeatStreak::advance` que nunca pasa de 1: 5 rojos, entre ellos
  `two_identical_calls_in_one_turn_nudges_the_second` y
  `streak_resets_when_a_different_signature_appears`.

**E2E.** No aplica: no hay cambio observable.

**ADP.** Nada que hacer.

## 96. Correr una llamada pasa a `run_call -> CallOutcome` (parallel tools, 2d)

**Qué cambió.** El segundo refactor de `AgentService::run` sin cambio de comportamiento,
después de la entrada 95. El cuerpo que ejecuta una llamada sale del loop y pasa, tal
cual, a `run_call(&CallCtx, &ToolCall) -> CallOutcome`: el frame Start, el chequeo de que
el nombre estaba ofrecido, la redirección de una tool lazy no cargada, el rechazo de un
nombre no ofrecido y el despacho al ejecutor. Después clasifica el centinela:
`Done(ToolResult)`, `Suspended(ToolResult, Value)` o `LoadAttachment(ToolResult, Value)`.
No escribe historia ni emite el Finish: eso lo hace quien la llama.
- `CallCtx` junta lo que `run_call` necesita de la iteración: el ejecutor, las tools de
  este pedido (`iteration_tools`), el catálogo lazy y `on_token`.
- En el loop, `Suspended` llama a `suspend` (entrada 95) y `LoadAttachment` corre el
  bloque de siempre, sin tocarlo. `Done` sigue como antes: el Finish y `record_result`.
- Así un grupo puede correr varias llamadas a la vez y escribir la historia después, en
  el orden del modelo.

**Tests.** Ninguno nuevo ni tocado: los 40 de `agent_service` pasan igual. Lib: 2905
passed, 0 failed, 74 ignored, como en la entrada 95.

**Mutación.** Sobre el código movido. Rojas y revertidas editando:
- `run_call` sin rechazar un nombre no ofrecido: rojo
  `a_call_to_a_tool_the_request_did_not_offer_never_reaches_the_executor`;
- el brazo `SUSPENDED` que nunca coincide: rojos
  `detects_suspended_tool_result_and_short_circuits` y los dos de suspensión en un batch.

**E2E.** No aplica: no hay cambio observable.

**ADP.** Nada que hacer.

## 97. Un grupo de llamadas `parallel` corre a la vez (parallel tools, 2e)

**Qué cambió.** `AgentService::run` arma las tandas de cada mensaje con la clave de
cadena y `plan_batches` (entrada 94). Una llamada sola corre igual que antes. Un grupo
de llamadas `parallel` seguidas corre en tres pasos:
1. el guard de repetición, sobre todo el grupo en el orden del modelo y con la misma
   regla de racha que en serie. Una repetición no corre: recibe el resultado de la
   primera de su racha, ya conocido o tomado después si esa corre en el grupo;
2. las cadenas a la vez (`buffer_unordered`, hasta `COLMENA_MAX_PARALLEL_TOOL_CALLS`,
   default 4, leído una vez por proceso), con las llamadas de una cadena en serie.
   Cada llamada emite su Finish cuando termina;
3. la historia, en el orden del modelo, con `answer_repeat` y `record_result`
   (entradas 95 y 96).
- Si una llamada del grupo suspende, su cadena para ahí: lo que sigue correría en el
  hilo que espera al humano. Lo que corrió se escribe, y `suspend` cierra con «NO se
  ejecutó» lo demás. Con dos suspensiones, hoy manda la primera en el orden del modelo
  (`TODO(parallel-suspend)`).
- Un `LoadAttachment` que salga de un grupo se contesta con su salida y un `warn`.
- Los frames de una repetición del grupo salen después de los del grupo. Un run cortado
  a mitad de grupo pierde los resultados ya terminados: la historia se escribe al final.

**Tests.** 7 nuevos, con el reloj de tokio en pausa y un ejecutor que duerme y anota
inicio, fin y pico: dos claves distintas se solapan, la misma clave no, la barrera, la
historia en el orden del modelo cuando la llamada de después termina primero, el tope
(5 claves, tope 2, pico 2), dos llamadas idénticas en un grupo y una suspensión en un
grupo. Los 40 de antes pasan sin tocarlos. Lib: 2912 passed, 0 failed, 74 ignored
(2905 en la entrada 96).

**Mutación.** Rojas y revertidas editando:
- `buffer_unordered(limit.min(1))`: 5 rojos (las dos claves, el tope, la historia y la
  suspensión);
- la historia en el orden de llegada: rojos el de la historia (`["c1", "c0"]`) y el de
  las llamadas idénticas;
- una cadena por llamada: rojos el de la misma clave y el de la suspensión.

**E2E.** No en este paso: el E2E de tiempo, con un grafo real, va aparte.

**ADP.** Sin código: los frames ya se asocian por `toolCallId` y `childScope`
(Startti/adp#855). Pero ninguna tool debe declarar `parallel` todavía: con dos
preguntas en un grupo, una queda sin hacer y su hijo suspendido.

## 98. Dos tests más del grupo, su invariante explícito y el modelo guionado compartido (parallel tools, 2f)

**Qué cambió.** Solo tests, sobre el grupo de la entrada 97:
- Los dos saltos de la pasada que escribe la historia del grupo (una repetición cuya
  gemela no terminó, una llamada que no corrió) dependen de que ya haya una suspensión
  anotada antes en esa pasada. Ahora es un `debug_assert!` con su comentario: si se
  rompe, un id queda sin resultado, y eso es un 400 en Anthropic y OpenAI.
- El test de la suspensión en un grupo le da 50 ms a `c2`. Antes `end c0` antes de
  `end c2` dependía de un empate entre dos esperas iguales, no de algo que el código
  garantiza.
- `ParallelTurnModel`, el modelo guionado del E2E de la entrada 91, pasa a un módulo
  compartido (`tests/parallel_turn_model/`) para el E2E que sigue. Recibe su lista de
  llamadas y guarda los ids de los resultados que leyó el modelo. El E2E de identidad
  afirma ahora ese orden, y dice por qué sus llamadas corren en serie: `Nota` no es
  `parallel`, así que es una barrera.

**Tests.** 2 nuevos en `agent_service`:
- `three_identical_parallel_calls_in_one_group_rescue_intra_turn`: tres llamadas
  idénticas en una misma cadena disparan el rescate. Mira la forma del segundo pedido
  (sin tools, y con el texto de síntesis al final), no solo el texto final;
- `a_groups_repeat_echoes_a_streak_that_started_in_an_earlier_batch`: la repetición en
  un grupo nuevo repite el resultado de la tanda anterior.

Lib: 2914 passed, 0 failed, 74 ignored (2912 en la entrada 97). E2E de identidad,
ignorado: 1 passed.

**Mutación.** Rojas y revertidas editando:
- sin `rescue = true` en el grupo: rojo el test del rescate. El test del rescate en serie
  no lo detecta con la misma mutación, porque solo mira el texto final;
- el eco de la racha anterior vacío (`Echo::Text(String::new())`): rojo el de la tanda
  anterior, sin la salida de `c1`.

**E2E.** `DATABASE_URL=postgres:///colmena_e2e_par SECURE_VALUES_KEY=... cargo test -p
colmena_dag_engine --test parallel_tool_identity -- --ignored`: 1 passed, con el orden
`call_clima`, `call_nota`, `call_precios` en la historia que leyó el modelo.

**ADP.** Nada que hacer.

## 99. E2E de un grupo de llamadas `parallel`, y la guía 19 (parallel tools, 2g)

**Qué cambió.** El E2E con tiempos del grupo de la entrada 97, y la documentación de
cómo corre un grupo.
- `tests/graphs/agents/parallel_tool_groups.json`: una tool `Run`, `parallel` y sin
  memoria, cuyo hijo es `entrada → python_script → salida`. El script duerme 2,3 s para
  `clima` y 2 s para `precios`. El modelo guionado (el módulo compartido de la entrada
  98) pide las dos en un mensaje.
- `tests/parallel_tool_groups.rs`: dos tests ignorados, en grupo y la línea base con
  `parallel: false`. Cada frame del SSE lleva su tiempo de llegada como comentario
  (`: +<ms>`).
- La guía 19 suma «Un grupo de llamadas `parallel` corre a la vez»: la barrera, las
  cadenas por clave de memoria (una tabla por `memory_mode`), el tope, los frames
  intercalados, la historia en el orden del modelo, el guard, la suspensión en un
  grupo, y por qué todavía no con varias preguntas. El bullet «Todavía en serie» ya no
  era cierto: ahora apunta a esa sección.
- `sse_events_reference.md` (`childScope`): los frames de un grupo se intercalan y se
  asocian por `toolCallId` y `childScope`, con frames reales del E2E.
- `node_configurations.json` y `node_as_tools_reference.json`: el campo `parallel` ya
  no dice que las llamadas corren en serie.
- El E2E de identidad, su grafo y el módulo compartido nombran este E2E.
  `EXPECTED_FILES` del corpus pasa de 331 a 332, 0/0/0.

**Tests.** Qué afirma el E2E en grupo, con el tiempo primero:
- del primer `tool-input-available` al último `tool-output-available` pasa menos de 1,5
  veces el hijo más corto, y cada hijo dura al menos 2 s;
- las fronteras `agent>Run#0` y `agent>Run#1` se solapan, y los frames de cada llamada
  llevan su `childScope`;
- `precios` cierra antes que `clima`, y la historia que lee el modelo es `call_clima`,
  `call_precios`.

La línea base afirma dos fronteras `agent>Run` sin `childScope`, la segunda después de
la primera, y al menos 2 veces el hijo más corto. `cargo test` completo: 60 suites, 3124
passed, 0 failed, 149 ignorados (147 antes, más estos dos). Lib: 2914, sin cambio.

**Mutación.** `buffer_unordered(limit.min(1))`: el E2E en grupo en rojo por el tiempo
(«the group took 4.411255458s, the shorter child 2.040661625s: not concurrent»), y la
línea base en verde. Revertida editando; verde.

**E2E.** `DATABASE_URL=postgres:///colmena_e2e_par SECURE_VALUES_KEY=... cargo test -p
colmena_dag_engine --test parallel_tool_groups --test parallel_tool_identity --
--ignored`: 3 passed.
- En grupo: 2,32 s, con un hijo de 2,02 s (1,15 veces). Los dos `tool-input-available`
  a los +153 ms; `precios` cierra a los +2171 ms y `clima` a los +2472 ms, aunque el
  modelo la pidió primero.
- En serie: 4,35 s, 2,15 veces el hijo más corto.
- La guía y la referencia SSE citan la corrida en la que se escribieron: 2,31 s en grupo
  y 4,34 s en serie, con los mismos frames a +30, +2045 y +2340 ms.

**ADP.** Nada nuevo respecto de la entrada 97: sin código, y ninguna tool declara
`parallel` todavía.

## 100. La nota de ADP del paso 2, y tres afirmaciones viejas sobre concurrencia (parallel tools, 2h)

**Qué cambió.** Solo documentación.
- La [nota de migración](adp_migration/2026-09-25-parallel-tool-calls.md) suma «Paso 2:
  un grupo de llamadas `parallel` corre a la vez», con su línea en el índice. Qué ve ADP
  en un grupo (frames intercalados, la historia en el orden del modelo, los frames de
  una repetición después de los del grupo), con los frames y los tiempos del E2E de la
  entrada 99. La frase del paso 1 «todavía corren una después de la otra» ahora dice
  que era cierta en ese paso.
- Tres afirmaciones viejas sobre concurrencia eran falsas. El orchestrator no cambia
  en este arco:
  - la guía 12 y `sse_events_reference.md` decían que el orchestrator corre en paralelo
    las tareas `parallel=true` de una fase. Las corre de a una (`orchestrator.rs`, el
    `for task in tasks_to_run` que espera cada sub-agente); `parallel` solo decide
    cuántas pendientes toma en una vuelta. La guía 12 suma una nota que apunta a lo que
    sí corre a la vez (guía 19);
  - `node_configurations.json` decía lo mismo del orchestrator;
  - la entrada 44 decía que las tool calls de un turno corrían concurrentes por un
    `JoinSet` en `llm.rs`: su nota al pie está desde la entrada 97.

**Tests.** No aplica: no cambia código. Siguen verdes `check_doc_links.py` (links y
números de sección) y `check_doc_counts.sh`; las anclas nuevas se revisaron aparte.

**Mutación.** No aplica.

**E2E.** No aplica; los frames y tiempos que cita la nota salen del E2E de la entrada 99.

**ADP.** Leer el paso 2 de la nota. No hace falta código, porque Startti/adp#855 ya
asocia los frames por `toolCallId` y `childScope`. Pero ADP no declara `parallel` en
ninguna tool hasta el paso que maneja varias preguntas en un grupo: hoy, con dos
preguntas en un grupo, una queda sin hacer y su hijo suspendido.

## 101. Una pregunta por turno dentro de un grupo `parallel` (parallel tools, 3a)

**Qué cambió.** Cuando una o más llamadas de un grupo `parallel` suspenden, el loop
espera a que termine el grupo y suspende el turno en **una** pregunta: la de la primera
llamada en el orden del modelo, no la primera en preguntar. Cierra el
`TODO(parallel-suspend)` de la entrada 97.
- **Las otras preguntas se cierran.** Método nuevo `ToolExecutor::close_suspended(call,
  outcome)`, con cuerpo por defecto vacío, que el loop llama una vez por cada otra
  llamada suspendida. `DagToolExecutor` pasa la fila del hijo a `FAILED` con
  `fail_if_suspended` y, solo si la cerró, a sus descendientes `SUSPENDED` con
  `fail_suspended_descendants`, como `close_refused`. No hace nada sin repositorio, con
  una tool que no es `subgraph` o sin `session_id` en la salida SUSPENDED del hijo, que
  ya lo traía (queda fijado por test). El modelo recibe
  `text/prompts/agent_loop/closed_by_parallel_suspend.md`, y el stream emite el
  `tool-output-available` de esa llamada con el mismo texto. El padre queda con un solo
  hijo `SUSPENDED`, el que `find_suspended_child` reanuda; con dos, elegía por
  `updated_at`.
- **«NO se ejecutó» va solo a lo que no corrió:** los sucesores de cadena de cualquier
  pregunta y las llamadas posteriores al grupo.
- **Cableado.** El repositorio de estado llega al ejecutor por un 7º parámetro de
  `HashMapNodeRegistry::new_with_secure_values` (`ColmenaEngine::new` pasa el suyo,
  `HashMapNodeRegistry::new` pasa `None`), y sigue por `LlmNode::with_state_repository`
  y `DagToolExecutor::with_state_repository`. Es un cambio de firma pública; en este
  repo solo lo llaman `engine.rs` y los tests.
- **Queda abierto hasta la entrada 102.** El hilo de memoria del hijo cerrado termina
  con su pregunta sin resultado. Re-correrlo, que es lo que pide el texto, manda ese id
  abierto: un 400 en Anthropic y OpenAI.

**Tests.** 5 nuevos en la lib, 2919 passed (2914 en la entrada 100):
- `agent_service`: dos preguntas en un grupo `c0..c6` (manda `c0` aunque `c3` preguntó
  antes, `close_suspended` una vez con `c3`, historia `[c2, c3, c5, c1, c4, c6]`, los
  Finish); el test de suspensión en grupo de la entrada 97, ampliado (no cierra nada);
- `dag_tool_executor`, `close_suspended`: cierra al hijo y a sus descendientes
  suspendidos, deja una fila que ya no está suspendida, no hace nada con una tool que no
  es `subgraph`;
- `run_use_case`: un hijo suspendido se nombra en su salida (`session_id`).

**Mutación.** Rojas y revertidas editando: el marcador también para la segunda pregunta;
sin llamar a `close_suspended` (`left: [] right: [("c3", …)]`); que mande la última
pregunta (`left: "c3" right: "c0"`); el sucesor de cadena sin marcador (rojos los dos
tests de grupo); sin la guarda de `subgraph`; los descendientes de una fila que esta
llamada no cerró; un hijo suspendido que no se nombra en su salida.

**E2E.** En las entradas 104 y 105. El cableado motor → registro → `llm_call` →
ejecutor solo lo cubre ese E2E.

**ADP.** Todavía ninguna tool debe declarar `parallel`: re-correr el hijo cerrado da 400
hasta la entrada 102. La guía 19 y la nota de migración dicen lo de antes hasta la
entrada 106.

## 102. Una corrida fresca contesta la pregunta que su hilo dejó abierta (parallel tools, 3b)

**Qué cambió.** Un `suspend` deja el id de su pregunta sin resultado a propósito: el
resume la encuentra por esa ausencia. Una pregunta que nunca se reanuda (la que un grupo
cerró en la entrada 101, o un resume rechazado por `close_refused`) queda abierta en su
hilo. Los hilos de las tools `persistent` y `dynamic` se comparten entre llamadas, así
que la llamada siguiente a ese agente arrancaba fresca sobre el mismo hilo y mandaba el
id abierto: un 400 en Anthropic y OpenAI, en esa request y en todas las siguientes.
- `AgentService::run`, en el camino fresco (con prompt o mensajes nuevos, nunca en el
  resume), contesta primero los ids sin resultado del turno en el que termina el hilo
  (un mensaje del asistente con `tool_calls` seguido solo de mensajes `tool`), y
  después agrega el prompt. La respuesta es el texto de
  `text/prompts/agent_loop/abandoned_question.md`, persistida como cualquier `tool`,
  con un `warn` por id. Vale para todos los agentes, no solo para Run My Agent.
- Un turno que el hilo ya dejó atrás (con un `user` o un `assistant` después) no se
  toca: un `tool` en ese lugar lo rechazan igual. La curación es idempotente.
- `unresolved_sibling_ids` y el helper nuevo `abandoned_call_ids` comparten
  `unresolved_ids`; lo de antes no cambia.
- El comentario del invariante del brazo «nunca corrió» del grupo dice ahora que, para
  el sucesor de una pregunta cerrada, `suspended` lo fijó la pregunta que quedó.
- El texto dice «pregunta», aunque la misma curación también contesta los ids de una
  corrida cortada. La entrada 103 lo cambia por uno neutro, en un archivo con otro
  nombre.

**Tests.** 6 nuevos en la lib, 2925 passed (2919 en la entrada 101):
- una corrida fresca con la forma de entrada de `llm_call` contesta la pregunta abierta
  una sola vez, en la request y en el hilo, y la request no lleva ningún id abierto;
- un hilo sin ids abiertos no cambia;
- el resume contesta con la respuesta, no con el marcador;
- un reintento (la primera corrida falla en el proveedor) no agrega un segundo marcador;
- 2 del helper: el turno en el que termina el hilo, y uno que el hilo dejó atrás.

**Mutación.** Rojas y revertidas editando: sin la curación (`left: ["ask"] right:
[]`); la curación también en el resume; la curación sobre un turno que el hilo dejó
atrás; la curación de todos los ids del turno, contestados o no.

**E2E.** En la entrada 105 (el escenario C re-corre el hijo cerrado en su hilo).

**ADP.** Sin código. Desde acá re-correr el hijo cerrado ya no da 400; la nota de
migración lo cuenta en la entrada 106.

## 103. `close_suspended` cierra solo bajo este run y loguea qué pasó; el marcador sirve para un run cortado (parallel tools, 3c)

**Qué cambió.**
- **Guarda de padre.** `DagToolExecutor::close_suspended` (entrada 101) tomaba el id
  del hijo de la salida de la tool y fallaba cualquier fila `SUSPENDED` con ese id.
  Ahora lee la fila primero (`get_by_id`) y la cierra solo si su `parent_session_id`
  es la sesión del ejecutor; sin session id no cierra nada.
- **Logs, solo con ids, nunca valores.** Sin session id del ejecutor, `error` (antes
  volvía en silencio). Fila no encontrada, `warn` «child row not found». Fila de otro
  padre, `warn` «not a child of this run». Una falla al leer o cerrar, `error`: el
  padre puede quedar con dos hijos `SUSPENDED`, y su resume va a fallar.
- **Marcador neutro.** Un Stop o el watchdog cortan un run después de guardar el
  mensaje del asistente, y dejan abiertos los ids que todavía no tenían resultado. La
  curación de la entrada 102 los contestaba llamando «pregunta» a cada uno. El texto
  pasa a `text/prompts/agent_loop/abandoned_tool_call.md` (antes
  `abandoned_question.md`), escrito para las dos causas, y la constante a
  `ABANDONED_TOOL_CALL_TEXT`.
- **El grafo del E2E entra acá, sin su test.** `tests/graphs/agents/parallel_tool_suspend.json`
  lo cubre el lint del corpus (333 archivos, 0/0/0), y `EXPECTED_FILES` de
  `corpus_noise` pasa de 332 a 333. Su comentario y el de `corpus_noise` ya nombran
  `parallel_tool_suspend.rs`, que llega en la entrada 104.

**Tests.** 4 nuevos en la lib, 2929 passed (2925 en la entrada 102):
- `close_suspended`: una fila `SUSPENDED` de otro padre queda como está, con su nieto;
  sin session id no cierra y loguea `error` con `tool_call_id` y `child_session_id`;
  una fila no encontrada loguea «child row not found». Los dos de logs capturan el
  `tracing` y verifican que el valor de la pregunta no aparece;
- la curación de un hilo cortado que termina en `A(x, y)` sin ningún `tool`: x e y se
  contestan una vez cada uno, y la request no lleva ningún id abierto.

**Mutación.** Rojas y revertidas editando: sin la guarda de padre (`left: Failed right:
Suspended`); la curación de solo el primer id abierto (`left: ["y"] right: []`). Los
dos tests de logs se vieron rojos antes del cambio.

**E2E.** En las entradas 104 y 105.

**ADP.** Sin código.

## 104. E2E: una pregunta en un grupo espera al grupo y se reanuda bajo su `childScope` (parallel tools, 3d)

**Qué cambió.** Solo tests.
- `src/libs/colmena/tests/caller_model/mod.rs`: un modelo guionado nuevo. Un guion
  recibe cada request (system y mensajes) y devuelve la respuesta (texto, o llamadas, un
  chunk por llamada con su índice) y cuánto esperar antes de darla. Es un módulo aparte
  de `parallel_turn_model`, así los otros dos E2E no cargan código muerto. Todavía no
  guarda las requests: eso llega con sus usos, en la entrada 105.
- `src/libs/colmena/tests/parallel_tool_suspend.rs`, sobre el grafo de la entrada 103,
  `#[ignore]` y `#[serial]`, contra un `ColmenaEngine` real y Postgres. El padre llama
  a `Run` (`parallel`, `dynamic` con el hilo fijo en `${agentId}`, como Run My Agent)
  una vez por agente en un mensaje. Cada hijo es un agente con memoria que puede
  preguntar con `Preguntar` (un `suspend`). El guion contesta según quién llama (PADRE
  o HIJO, por el system prompt).
- **Escenario A, una pregunta y un hermano que termina.** El modelo pide primero a
  `beta` (termina a los 600 ms) y después a `alfa` (pregunta enseguida), así que la
  pregunta que queda es `Run#1`, no el k = 0 por defecto. Turno 1: la pregunta de `alfa`
  sale antes que el `tool-output-available` de `beta` (`childScope` `Run#0`, «beta:
  hecho»), y ese antes que el `finish` suspendido en la pregunta de `alfa`: el turno
  esperó al grupo. `call_alfa` no tiene `tool-output-available`, y en `dag_runs` quedan
  `alfa` `SUSPENDED` y `beta` `COMPLETED`. Turno 2, con la respuesta: el padre termina
  con «Listo.», cada frame del hijo reanudado viene bajo `agent>Run#1>`, y los dos hijos
  quedan `COMPLETED`.

**Tests.** La lib no cambia (2929). `SECURE_VALUES_KEY=… DATABASE_URL=postgres:///colmena_e2e_par
cargo test -p colmena_dag_engine --test parallel_tool_suspend -- --ignored`: 1 passed.

**Mutación.** El resume siempre con k = 0 (`pending_call_to_resume`, en `llm.rs`): rojo
A, con el hijo reanudado bajo `agent>Run#0>hijo`. Con la pregunta en `Run#0`, A pasaba
igual; por eso el orden de sus llamadas. Revertida editando.

**E2E.** Este. B, C y los chequeos de lo que recibe cada modelo, en la entrada 105.

**ADP.** Sin código.

## 105. Endurecimiento: `${VAR}` se expande solo en la configuración del autor

**Qué cambia.** Un valor que llega por `inputs` (un edge, el estado global, una fila de
`for_each`) ya no expande plantillas `${VAR}`, salvo que un despacho con provenance (el de
tools o `for_each`) marque su puntero como un valor `fixed` del autor. `EnvPolicy::from_inputs`
sin la clave `__colmena_env_trusted_paths` (o con una malformada) es `Restricted(∅)`; se
borra `Legacy`. `config` sigue expandiendo siempre, así que el `bearer_token: "${TOKEN}"` del
autor funciona igual. El path multipart de `http_request` aplica la misma política que el JSON
a body, headers y `bearer_token`/`authorization` de `inputs`. `for_each` manda por fila los
punteros de los `fixed` de su `target` (`trusted_pointers`, la misma regla del despacho de
tools).

**Tests.** `run_use_case.rs::graph_http_payload_tests` (grafo real `trigger_webhook` →
`http_request`, mocks locales, variables solo de test): un valor aplanado llega literal y el
`${VAR}` de `config` se expande; un edge que nombra el campo fija `base_url` y su valor no se
expande. `for_each.rs::http_target_env_tests`: la fila llega literal, el `fixed` se expande.
En `http.rs`, multipart con punteros vacíos: body, header y bearer de `inputs` literales,
header de `config` expandido; y sin clave nada de `inputs` se expande (también en
`env_provenance.rs`).

**ADP.** Sin cambios de API. Un grafo que ponía `${VAR}` en un valor que llega por un edge
(o en una fila de `for_each` fuera de sus `fixed`) ahora lo manda literal: el lugar de un
secreto es `config` o un `fixed`. Ninguno en `tests/graphs`.
**Estado.** done.
