# 50. Logging y observabilidad — contrato de namespaces y payloads

> Documenta el contrato de tracing que reemplaza los `println!`/`eprintln!`
> directos a stdout en `python_node.rs`, `sql.rs` y `orchestrator.rs`
> (finding #30 del [ledger de auditoría](../agent_context/audit/FINDINGS_LEDGER.md)).
> Este contrato es la referencia que consume el worker de ADP para filtrar
> logs por `RUST_LOG` en Cloud Logging.

## TL;DR

- Todo el logging de colmena usa [`tracing`](https://docs.rs/tracing), nunca
  `println!`/`eprintln!` fuera de tests.
- Los eventos operativos (metadata segura: tamaños, ids, flags) viven bajo
  `colmena::<nodo>`. El contenido crudo controlado por el usuario o el LLM
  (código Python, SQL, planes del orchestrator) vive bajo un target separado
  `colmena::payload::<tipo>`.
- Emitir un payload requiere **dos condiciones independientes** a la vez: el
  `EnvFilter` debe habilitar el target `colmena::payload::*`, Y la variable
  `COLMENA_LOG_PAYLOADS` debe estar activa al arrancar el proceso. Ninguna de
  las dos por sí sola alcanza.
- La librería (`src/libs/colmena/src/`) **nunca** instala un subscriber ni lee
  `RUST_LOG`. Cada binario decide su propia política de logging.

## Principio arquitectónico: "la librería emite, la aplicación decide"

Colmena es una librería consumida por múltiples hosts (el binario propio
`dag_engine`, el worker de ADP en
`apps/service/ia/platform/worker/src/main.rs:56`, y potencialmente otros
integradores). Ningún módulo dentro de `src/libs/colmena/src/` instala un
`tracing_subscriber`, llama a `EnvFilter::from_env`, ni lee `RUST_LOG`
directamente — eso acoplaría la librería a una política de logging que solo
la aplicación que la embebe puede decidir con propiedad (destino de los logs,
verbosidad, agregación en Cloud Logging, etc.).

En consecuencia:

- El binario `dag_engine` (`src/libs/colmena/src/dag_engine/main.rs`) es
  responsable de instalar su propio subscriber `tracing_subscriber::fmt`
  con un `EnvFilter`, una única vez, antes de despachar cualquier subcomando
  (`run`, `serve`, etc.).
- El worker de ADP instala el suyo de forma independiente
  (`apps/service/ia/platform/worker/src/main.rs:56`), con su propia política
  de verbosidad y su propio destino (Cloud Logging estructurado).
- Un integrador nuevo que embeba colmena como librería es libre de no
  instalar ningún subscriber — en ese caso los eventos de `tracing` simplemente
  no tienen consumidor y se descartan, sin que la librería se entere ni le
  importe.

`.env` en el binario propio se carga vía `dotenvy::dotenv()` al inicio de
`main()`, antes de resolver el filtro — así `RUST_LOG` y
`COLMENA_LOG_PAYLOADS` pueden fijarse en el `.env` local para desarrollo sin
tener que exportarlos manualmente en cada shell.

## Taxonomía de targets

Todo target de `tracing` en colmena vive bajo el prefijo `colmena::`. Hay dos
familias, con semántica distinta:

| Target | Qué transporta | Contenido |
|---|---|---|
| `colmena::python_node` | Metadata del evento de ejecución del nodo `python_script` | `code_len`, `sandbox_mode`, `timeout_secs` — nunca el código en sí |
| `colmena::sql` | Metadata del evento de ejecución del nodo `sql_query` | `query_len`, `session_id`, `max_rows`, errores de ejecución — nunca la consulta en sí |
| `colmena::orchestrator` | Metadata del ciclo del orchestrator (planner/critic) | `task_count`, `phase_count`, `phase` — nunca el plan renderizado |
| `colmena::payload::python_code` | Código Python crudo del nodo `python_script` | El body completo del script, sin truncar |
| `colmena::payload::sql_query` | SQL crudo del nodo `sql_query` | La consulta completa, sin truncar |
| `colmena::payload::planner_plan` | Plan renderizado del orchestrator (líneas `[agent]: task → ctx`) | Texto generado por el LLM, sin truncar |

`colmena::payload::planner_plan` es el tercer target de payload, además de
los dos que documentaba la propuesta original (`python_code`, `sql_query`).
Se añadió porque el bloque de log del orchestrator también interpola texto
autoría del LLM (`task`/`context` renderizados) — sin este target dedicado,
un operador que apagara `colmena::payload=off` seguiría viendo ese contenido
a través de un `debug!` genérico, lo cual rompería la garantía central de
este contrato.

### Por qué los payloads comparten el prefijo `colmena::payload::*`

Todos los targets de payload cuelgan de `colmena::payload::*` a propósito:
una sola directiva de `EnvFilter`, `colmena::payload=off`, apaga los tres de
una vez, sin tener que enumerar cada uno. Esto es lo que hace posible la fila
"sensible" de la matriz de abajo: se puede mantener trazabilidad completa del
flujo (`colmena=trace`) mientras se silencia únicamente el contenido crudo.

### Qué campos son seguros en los eventos (allow-list)

Los eventos sobre `colmena::<nodo>` (no payload) deben limitarse a metadata
operativa:

- **Permitido**: longitudes (`code_len`, `query_len`), flags/modos
  (`sandbox_mode`), ids de sesión o nodo, conteos (`task_count`,
  `phase_count`), booleanos, nombres de fase.
- **Prohibido**: código Python crudo, SQL crudo, valores literales de filas,
  texto libre generado por el LLM.

## El gate doble sobre payloads

Emitir contenido bajo `colmena::payload::*` exige **ambas** condiciones a la
vez, evaluadas de forma independiente:

1. **El filtro habilita el target.** El `EnvFilter` del proceso debe permitir
   el nivel `trace` en `colmena::payload::*` (o en su target específico).
2. **La variable de entorno está activa.** `COLMENA_LOG_PAYLOADS` debe
   resolverse a verdadero (`1`/`true`/`yes`/`on`) al momento de arrancar el
   proceso — el guard se cachea de forma perezosa la primera vez que se
   consulta.

Ninguna condición por sí sola es suficiente. Esto es intencional: protege
contra dos errores operativos distintos —

- **Reflejo de `RUST_LOG=trace`**: un operador de guardia sube el nivel de
  log a `trace` para depurar un problema no relacionado y, sin la variable de
  entorno, no expone ningún payload por accidente.
- **Variable heredada de otro entorno**: `COLMENA_LOG_PAYLOADS=1` queda
  seteada (por ejemplo, copiada de un `.env` de desarrollo) pero el filtro de
  producción nunca habilita `colmena::payload`, así que tampoco se expone
  nada.

| Caso | `RUST_LOG` | `COLMENA_LOG_PAYLOADS` | ¿Aparece el payload? |
|---|---|---|---|
| a | `info` | unset | No |
| b | `colmena=trace` | unset | **No** — el guard es independiente del filtro |
| c | `colmena=trace,colmena::payload=off` | `1` | **No** — la directiva del filtro es independiente del guard |
| d | `colmena::payload::python_code=trace` (o `colmena=trace`) | `1` | Sí |

## Matriz de configuración por entorno

| Entorno | `RUST_LOG` | `COLMENA_LOG_PAYLOADS` | Resultado |
|---|---|---|---|
| Producción | `info` (sin cambios) | unset | Solo eventos operativos; cero contenido de usuario **en el stream de logs** (ver "Qué NO documenta este contrato") |
| Develop | `colmena=trace` | `1` | Trazabilidad completa, incluyendo código literal y SQL |
| Develop, sesión sensible | `colmena=trace,colmena::payload=off` | cualquiera | Trazado completo del flujo, payload suprimido |
| CLI local | default `info`; `--verbose` (o `COLMENA_VERBOSE=1`) → `colmena=debug` | opt-in | Igual que producción, reproducible localmente |

Notas sobre la fila de CLI local: `--verbose` sube el nivel del filtro a
`colmena=debug` (paridad con la vista de plan del orchestrator que ya existía
antes de este cambio), pero **no** habilita ningún target de payload por sí
mismo — sigue haciendo falta `COLMENA_LOG_PAYLOADS=1` explícito para ver
contenido crudo, incluso en modo verbose.

## Resolución del filtro en el binario `dag_engine`

`RUST_LOG`, cuando está definido y no vacío, siempre gana. Si no está
definido, el default es `info`; `--verbose` — o `COLMENA_VERBOSE=1`, que el
help del flag documenta como equivalente — lo sube a `colmena=debug`. La
inicialización usa `try_init()` (no entra en pánico si ya hay un subscriber
instalado), y ocurre una sola vez antes de despachar cualquier subcomando —
tanto `run` como `serve` comparten la misma resolución.

## Límite honesto de esta garantía

Esta es una garantía **de configuración, con dos gates deliberados e
independientes** — no una imposibilidad a nivel de compilación. Si un
operador fija explícitamente `RUST_LOG=colmena=trace` junto con
`COLMENA_LOG_PAYLOADS=1`, el sistema expone el contenido crudo por diseño —
esa es precisamente la postura intencional de `develop`. Nada en este
contrato impide que alguien con acceso a las variables de entorno del
proceso decida exponer payloads; lo que el contrato garantiza es que **no
ocurre por accidente** con una sola de las dos palancas.

Tampoco es una barrera de seguridad contra un atacante con capacidad de fijar
variables de entorno del proceso — es un mecanismo de higiene operativa
pensado para separar "quiero ver el flujo de ejecución" de "quiero ver el
contenido exacto que procesó el usuario o el LLM", y para que esa segunda
decisión requiera un acto explícito y doble.

## Excepción: errores de SQL siguen visibles en producción

El error de ejecución de SQL (`sql.rs`, sitio de la conexión a base de
datos) se mantiene visible en la postura default de producción
(`RUST_LOG=info`) como `warn!` sobre `colmena::sql` — es una condición de
error genuina, no contenido de payload, y perderla en producción degradaría
la observabilidad operativa. Nota: el string de error puede seguir
conteniendo fragmentos del SQL fallido si el driver los incluye en el mensaje
de error — esa redacción específica queda fuera de este contrato y está
registrada como un ítem abierto en el ledger de auditoría (ver
`docs/agent_context/audit/FINDINGS_LEDGER.md`).

## Qué NO documenta este contrato

- **No cubre el stream de eventos SSE.** Los frames `node-start` incluyen
  `config` e `inputs` verbatim, de modo que el cuerpo de código de un nodo
  `python_script` viaja dentro del evento aunque este contrato lo suprima del
  stream de logs. Verificado E2E: con la postura default (`RUST_LOG` e
  `COLMENA_LOG_PAYLOADS` sin fijar) el `println!` desaparece pero el código
  sigue apareciendo en el frame `node-start`. Ese stream viaja por HTTP/Redis
  hacia el cliente que ejecuta el grafo — el worker de ADP no imprime eventos
  a stdout (verificado: cero `println!`/`eprintln!` en su árbol de fuentes),
  así que **no alcanza Cloud Logging**; en el CLI se imprime en la terminal
  del operador, que está mirando su propia corrida. Es un canal distinto, con
  semántica y audiencia distintas: su filtrado se registra como ítem aparte
  en el ledger de auditoría y no forma parte de esta garantía.

- No cubre los ~31 archivos que ya usaban `tracing` antes de este cambio
  (findings #22, #29 quedan fuera de alcance).
- No introduce ningún nuevo mecanismo de enmascarado tipo `colmena_log!`
  condicionado por flag — esa alternativa fue evaluada y descartada a nivel
  de diseño porque sigue dependiendo de que el desarrollador recuerde usarla
  en cada sitio, mientras que el macro `payload_trace!` hace que el guard sea
  estructuralmente imposible de omitir en el call site.
- No cambia si `attachment_gc` honra `RUST_LOG`: ya lo hacía. Verificado
  contra el fuente de `tracing-subscriber 0.3.20` (`fmt/mod.rs`): sin la
  feature `env-filter`, `try_init()` construye igualmente un filtro `Targets`
  a partir de `RUST_LOG`. Promover la feature a la dependencia principal
  cambia ese binario de `Targets` a `EnvFilter` — sintaxis de directivas más
  rica, mismo respeto por la variable — no lo hace pasar de sordo a oyente.

## Ver también

- [Nodo Python Script](./26_python_node.md)
- [Nodo SQL Query](./23_sql_node.md)
- [Arquitectura del Orchestrator](./20_orchestrator_architecture.md)
- [Testing](./05_testing.md) — convención `#[ignore]` para tests que leen
  variables de entorno, aplicable también a tests de este contrato.
