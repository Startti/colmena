# Un argumento del modelo ya no elige el grafo que corre un hijo

**Acción de ADP: ninguna de código; subir el motor.** "Run My Agent" compila su fuente
como `child_graph_ref` fijo, que es justo lo que el modelo ya no puede pisar.

## Qué cambia

Hasta ahora, un `child_graph_inline` o `child_graph_path` que el modelo agregara a una
llamada a "Run My Agent" le ganaba al ref fijo: el motor no consultaba al resolvedor
del worker —ni su `forbidden`— y corría el grafo del modelo, con `python_script` sin
sandbox y `${VAR}` del entorno del proceso. Ahora esas claves se descartan si la tool
no las ofrece como parámetro, con un aviso en stderr que nombra la clave, nunca el valor.
Una llamada normal no cambia; una tool que declare una fuente como parámetro visible
sigue recibiéndola (no se verificó desde este repo si ADP compila alguna).

## Qué se rompe si se ignora

Nada. Sin subir el motor, el modelo puede seguir eligiendo el grafo que corre el worker.
Aparte: el despacho todavía no compara el nombre de la tool con las expuestas (una
llamada a `python_script` por su nombre corre aunque el agente no la tenga); lo cierra
[Una tool no ofrecida no corre](2026-09-24-unoffered-tool-refused.md).
