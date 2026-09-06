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

## 26. Allowlist de hosts para MCP, opcional y apagada por default

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
