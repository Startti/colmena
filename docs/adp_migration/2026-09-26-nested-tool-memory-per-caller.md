# El hilo de memoria de una tool es de quien la llama

**Acción de ADP: ninguna de código.** Hay que subir el pin de Colmena en el worker
(`apps/service/ia/platform/worker/Cargo.toml`), actualizar tres comentarios y, antes de
promover a prod, correr la medición de abajo en solo lectura. No hay migración de base.

## Qué cambia

Una tool con memoria `persistent` o `dynamic` guarda su conversación en
`llm_node_history` bajo un `node_id`. Hasta v0.20.1 ese `node_id` era
`tool/<tool_name>[/<thread_id>]` para cualquiera que llamara la tool. Ahora es de quien
la llama:

- **Desde la raíz, igual que hoy.** Auto y `Run_My_Agent` siguen en
  `tool/Run_My_Agent/<agentId>/…`. Tampoco cambian los hijos de un `subgraph` de nivel de
  grafo ni los agentes de un `orchestrator` en la raíz.
- **Desde dentro de un hijo invocado como tool** (el `llm_call` que llama tiene un
  `node_id_path` que empieza con `tool/`), el hilo cuelga de su camino:
  `<caller>/tool/<tool_name>[/<thread_id>]`. Por ejemplo, un agente A que corre Auto y
  tiene un asset S con `memoryMode` guarda a S en `tool/Run_My_Agent/A/<llm de A>/tool/S`,
  no en `tool/S`.
- **Un camino de quien llama de más de 256 bytes se acota:** la clave cuelga de
  `tool/~<32 hex>` (los primeros 16 bytes del SHA-256 del camino entero) en vez del
  camino. Sigue empezando con `tool/`, y el nivel siguiente vuelve a acotar, así que la
  clave no pasa de 455 bytes a ninguna profundidad. En dev ninguna clave supera hoy 256
  bytes (la más larga mide 123), así que ninguna clave existente cambia por esto.
- **`stateless` no cambia:** `tool/<tool_call_id>`, desde cualquier nivel. Los hijos del
  creador son `stateless`, así que sus claves quedan iguales.
- `list_threads` lista solo los hilos de quien llama.
- **Una respuesta continúa la conversación donde se hizo la pregunta.** Un `llm_call`
  que se reanuda con la respuesta corre bajo el `_conversation_key.node_id` que guardó su
  salida `SUSPENDED` en `dag_runs.all_outputs`. Una pregunta que un hijo anidado hizo con
  v0.20.1, bajo `tool/<tool_name>/…`, se contesta sobre esa historia después de subir el
  pin. El scrub en reposo de ADP (`apps/api/scripts/lib/dag-run-at-rest.ts`) conserva ese
  campo.

Con esto se cierran dos casos que hoy se pueden armar desde el canvas:

- dos agentes que Auto corre en el mismo chat y cuelgan el mismo asset con memoria ya no
  comparten su conversación;
- un agente que tiene S y también un asset Q cuyo grafo trae S tampoco la comparte.

## Qué ve ADP

- **Nada en el stream.** El `path` de los frames y el `childScope` no dependen de la clave
  de memoria.
- **En `llm_node_history`,** filas nuevas con forma `tool/…/tool/…` (o
  `tool/~<hex>/tool/…`, con un camino de quien llama largo). ADP no lee esa tabla
  en runtime: el modelo Prisma `LlmNodeHistory` solo borra en cascada. Los scripts y evals
  que distinguen la raíz (id pelado) de lo anidado (`node_id LIKE 'tool/%'`), como
  `measure-root-routing.ts`, siguen valiendo.
- **Qué hilos cambian de clave:** los `persistent`/`dynamic` llamados desde dentro de un
  hijo. Medido en dev el 2026-09-26, en solo lectura: 0 filas. Una conversación que ya
  existía bajo la clave vieja no se mueve; la próxima llamada anidada empieza un hilo nuevo
  bajo la clave nueva.
- **El hilo dura lo que dura el camino de quien llama.** Dentro del hijo de una tool
  `stateless` (`tool/<tool_call_id>/…`), una tool con memoria recuerda dentro de esa
  llamada, no entre llamadas.

## Comentarios que hay que actualizar en ADP

- `apps/api/src/agents/groups/application/memory-mode.ts`, la tabla de claves del
  encabezado. Sumar que desde dentro de un hijo invocado como tool, `persistent` y
  `dynamic` cuelgan del camino de quien llama.
- `apps/api/scripts/agent-roles/role-shared.ts`, el comentario de `memoryMode` en el ref
  `subagent` («Colmena la keyea por `tool/<nombre>` … los diez padres compartirían una sola
  conversación»). Ya no vale para padres anidados: cada padre que es hijo de una tool
  tiene su propio hilo. Si el padre es `stateless`, ese hilo dura una llamada del padre.
- `apps/api/scripts/agent-roles/creator-v2-defs.ts`, el comentario sobre `Document
  agent`/`verify agent` («declararles memoria acá compartiría una sola conversación entre
  sus diez padres»). Mismo matiz. Relajar esa regla del creador es una decisión de
  producto, no de este cambio.

## Límites conocidos

- **Un `llm_call` invocado directamente como tool no guarda su clave.** Solo se honra la
  clave de un `llm_call` que es nodo de un grafo, por ejemplo el de un hijo de una tool
  `subgraph`. Un `llm_call` con memoria invocado como tool (`tool/<H>`), llamado desde
  dentro de un hijo, que preguntó con v0.20.1, deriva `<caller>/tool/<H>` al reanudar. Su
  salida `SUSPENDED` nunca se guardó en `all_outputs`, y bajo la clave nueva el hilo está
  vacío. No es un reinicio silencioso: el resume corre sin mensajes, falla con `Empty
  message list`, y quien llama recibe un error de tool visible (`Error executing node
  llm_call: …`) como resultado. La respuesta del usuario se pierde. Pasa una sola vez, a
  través de la subida del pin. En dev hay 0 cadenas así, y ADP compila los assets como
  `subgraph`. La consulta 3 de abajo mide cuántas quedan en prod.
- **Una clave acotada no se lee.** No dice quién llamó: para saberlo, se compara su
  digest con el SHA-256 de los `node_id` de la sesión. A cambio, la profundidad ya no
  tiene tope por la clave (sin acotar, cada nivel sumaba hasta unos 230 bytes, y los
  ~2704 bytes por entrada del índice btree de `llm_node_history` ponían el techo cerca de
  11 niveles). Medido en dev: profundidad máxima 4, clave más larga 123 bytes.

## Medición en prod antes de promover

En solo lectura, contra la base de prod. La consulta 1 cuenta los hilos con nombre que
alguien llamó desde dentro de un hijo, que son los que cambian de clave. Atribuye cada
fila `tool/<nombre>/…` a los mensajes del asistente de la misma sesión que llamaron una
tool con ese nombre; eso sobreestima cuando dos niveles usan hilos distintos. La 2 cuenta
las cadenas suspendidas a profundidad 2 o más, las que el resume tiene que cruzar. La 3
cuenta la exposición al primer límite: hilos con la forma de un `llm_call` invocado como
tool bajo la clave vieja (`tool/<H>` o `tool/<H>/<hilo>`), que alguien llamó desde dentro
de un hijo y cuya última fila es un `assistant` con `tool_calls` sin `tool` posterior (una
pregunta abierta). Es una cota superior: `tool/<nombre>/<nodo>` también es la forma del
hijo de un `subgraph` `persistent`, que sí se honra, y una llamada cortada por un Stop
deja la misma última fila.

```sql
BEGIN READ ONLY;

-- 1. Hilos con nombre (persistent/dynamic) que alguien llamó desde dentro de un hijo.
WITH llamadas AS (
  SELECT h.agent_session_id, h.node_id AS quien_llama, c->'function'->>'name' AS tool
  FROM llm_node_history h, jsonb_array_elements(h.tool_calls) c
  WHERE h.role = 'assistant' AND jsonb_typeof(h.tool_calls) = 'array'
)
SELECT count(DISTINCT (t.agent_session_id, t.node_id)) AS hilos,
       count(DISTINCT t.agent_session_id) AS sesiones, count(*) AS filas
FROM llm_node_history t
WHERE t.node_id LIKE 'tool/%'
  AND EXISTS (SELECT 1 FROM llamadas l
              WHERE l.agent_session_id = t.agent_session_id
                AND l.tool = split_part(t.node_id, '/', 2)
                AND l.quien_llama LIKE 'tool/%');

-- 2. Cadenas suspendidas a profundidad 2 o más (el padre de la fila tiene padre).
SELECT count(*) AS filas, count(DISTINCT r.agent_session_id) AS sesiones
FROM dag_runs r JOIN dag_runs p ON p.session_id = r.parent_session_id
WHERE r.status = 'SUSPENDED' AND p.parent_session_id IS NOT NULL;

-- 3. Límite 1: preguntas abiertas en el hilo de un llm_call invocado como tool, bajo la
--    clave vieja, que alguien llamó desde dentro de un hijo.
WITH llamadas AS (
  SELECT h.agent_session_id, h.node_id AS quien_llama, c->'function'->>'name' AS tool
  FROM llm_node_history h, jsonb_array_elements(h.tool_calls) c
  WHERE h.role = 'assistant' AND jsonb_typeof(h.tool_calls) = 'array'
), ultimas AS (
  SELECT DISTINCT ON (h.agent_session_id, h.node_id)
         h.agent_session_id, h.node_id, h.role, h.tool_calls
  FROM llm_node_history h
  WHERE h.node_id ~ '^tool/[^/]+(/[^/]+)?$'
  ORDER BY h.agent_session_id, h.node_id, h.created_at DESC, h.id DESC
)
SELECT count(*) AS hilos, count(DISTINCT u.agent_session_id) AS sesiones
FROM ultimas u
WHERE u.role = 'assistant' AND jsonb_typeof(u.tool_calls) = 'array'
  AND u.tool_calls <> '[]'::jsonb
  AND EXISTS (SELECT 1 FROM llamadas l
              WHERE l.agent_session_id = u.agent_session_id
                AND l.tool = split_part(u.node_id, '/', 2)
                AND l.quien_llama LIKE 'tool/%');

ROLLBACK;
```

La 2 no ve el primer límite. Un `llm_call` invocado como tool corre dentro del nodo de
quien lo llama y no crea fila en `dag_runs`: la cadena suspendida termina en el hijo que lo
llamó, que puede estar a profundidad 1. Por eso la 3 lo busca en `llm_node_history`.

Si la 3 da más de 0, hay que mirar qué tool es cada hilo. Los que son un `llm_call`
invocado como tool (no un asset) son los del primer límite: al reanudar, quien llama
recibe un error de tool en vez de la respuesta, una vez.

Guía: [19 — De quién es el hilo](../developer_guide/19_nested_agents_and_subgraphs.md#de-quién-es-el-hilo).
