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
| `NO_CATALOG_COVERAGE` | info | El tipo de nodo no tiene entrada en el catálogo: **no se revisó** |

Los `code` son estables. Cualquier consumidor (la salida JSON, una UI sobre los
bindings) debe ramificar sobre ellos, nunca sobre el texto del mensaje.

## Las decisiones que evitan el ruido

Un linter con falsos positivos se ignora. Cuatro reglas existen sólo para eso, y
cada una salió de medir contra los 301 grafos de ejemplo del repo.

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

Sobre los 301 grafos de ejemplo del repo (298 parsean), el linter deja **252
limpios** y produce **80 hallazgos**. Cada categoría restante fue auditada contra
el código: no hay falsos positivos conocidos. Si agregás una regla, medí de nuevo
— la primera versión de este linter producía 178 hallazgos de los que 132 eran
ruido, y sin medir eso no se nota.

## Limitaciones conocidas

- **El catálogo se mantiene a mano.** Los tests garantizan que la *lista de
  tipos* no se desvíe, pero no que los *campos* de cada tipo sigan al código. El
  plan de fase 2 es declarar el schema en el propio `ExecutableNode` y pasar a
  **generar** este JSON.
- **`tool_configurations` no se revisa a nivel de campo.** Lo que ya valida
  `Graph::validate()` (`memory_mode`, bloque `mcp`, `node_schema`) sigue siendo
  la única cobertura ahí.
- **Cuatro tipos condicionales se dan por disponibles.** El linter revisa el
  grafo contra lo que el motor *puede* ejecutar, no contra el cableado de un
  despliegue concreto.
- **Las claves de la raíz del grafo no se revisan.** En los 301 grafos del repo,
  todas las claves raíz no declaradas son anotaciones; marcarlas enterraría los
  hallazgos que importan.
- **`NO_CATALOG_COVERAGE` no se alcanza desde la CLI.** `LintContext::from_catalog()`
  deriva los tipos ejecutables del propio catálogo, así que un tipo sin entrada
  sale como `UNKNOWN_NODE_TYPE`. Los tests de cobertura del registry impiden que
  eso pase dentro del repo; un consumidor externo que use ese contexto recibiría
  el mensaje equivocado. Para obtener el comportamiento documentado hay que
  construir el `LintContext` con la lista real del registry.
