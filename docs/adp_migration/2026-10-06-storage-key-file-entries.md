# Entradas de `files[]` con solo `storage_key` (archivos tabulares grandes)


> **Orden de despliegue (obligatorio).** Despliega primero el `attachment_gc` nuevo y
> solo después enciende `COLMENA_LARGE_TABULAR` en el motor. Un `attachment_gc`
> **anterior** que corra contra filas escritas por un motor nuevo borraría el
> objeto del usuario: borra `storage_key` de toda fila vencida y el adaptador de
> callback reenvía ese borrado a ADP. Ninguna defensa del lado del motor sustituye
> este orden (ver «Por qué no hay otra defensa» abajo).
>
> **Una excepción a «con el interruptor apagado todo es como antes».** `attachment_gc`
> ahora borra **fila por fila** (`sesión + documento + proveedor`) en vez de borrar
> todas las filas de un documento cuando juzga una. Es un cambio deliberado que no
> depende del interruptor y solo se nota en un documento con filas de varios
> proveedores: la fila que no se juzgó ya no desaparece junto con la otra (antes
> quedaba su copia huérfana). Con una sola fila por documento el resultado es el de
> siempre. La regla de propiedad del registro (la clase de una fila no cambia)
> no altera ninguna escritura mientras no existan filas `host_storage_ref`, y no hay
> columnas nuevas: los contadores del GC son solo de log.

**Acción de ADP:** ninguna al compilar. Cuando ADP quiera usar la ruta de archivos
grandes tendrá que emitir la entrada descrita abajo y activar el interruptor
`COLMENA_LARGE_TABULAR` del motor; con el interruptor apagado (el valor por defecto)
el motor se comporta exactamente como antes, para cualquier tamaño.

## Qué cambia

- Con el interruptor encendido, una entrada de `files[]` que trae únicamente
  `storage_key` (sin `data`, `url` ni `path`), de tipo `text/csv` o xlsx y con
  `size_bytes` **estrictamente mayor** a 52 428 800 bytes (50 MiB) deja de omitirse:
  el motor lo reconoce como archivo grande (`FileSource::StorageRef`) sin descargar
  el archivo ni subirlo al proveedor. Registrarlo como adjunto llega en un cambio
  posterior de la misma cadena; hasta entonces el modelo todavía no lo ve. Los campos que lee el motor son `id`, `mime_type`,
  `filename`, `size_bytes` y `storage_key`.
- **Qué debe enviar ADP, exactamente.** `mime_type` igual a `text/csv` o a
  `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet` (se ignoran
  mayúsculas y parámetros como `; charset=utf-8`) y `size_bytes` entero, tomado de
  los metadatos reales del objeto, **mayor** a 52 428 800. No sirven
  `application/csv`, `text/plain`, `application/vnd.ms-excel` (`.xls`) ni
  `application/octet-stream` aunque el nombre termine en `.csv` o `.xlsx`: el motor
  no deduce el tipo por el nombre.
- Cualquier otra entrada con solo `storage_key` no se registra como archivo grande
  (no se rechaza ni falla el turno): interruptor apagado, otro tipo de archivo,
  `size_bytes` ausente o menor o igual a 50 MiB (exactamente 50 MiB es pequeño). Las
  demás entradas no cambian de índice. Con el interruptor **apagado** se omite en
  silencio, como siempre. Con el interruptor **encendido** el motor escribe en el
  log el motivo concreto y avisa al modelo en cada turno, en el sufijo del mensaje de
  sistema, bajo «Attachments not delivered» (nombre del archivo y motivo, nunca la
  llave; hasta 10 avisos), para que un archivo mal clasificado por el emisor no
  desaparezca sin rastro.
- Si la entrada trae además `data`, `url` o `path`, se interpreta como hasta ahora
  (prioridad `data > url > path`): la llave solo se usa cuando no hay ninguna otra
  fuente. El emisor debe omitir `data` y `url` para los archivos grandes.
- Rust: `llm::domain::FileSource` tiene una variante nueva, `StorageRef(String)`. Un
  `match` exhaustivo sobre ese tipo deja de compilar. Comprobado en el repo de ADP
  (no en este): `git grep FileSource -- apps` no devuelve ningún resultado, así que
  no hay `match` que actualizar.
- **Filas de `conversation_attachments`.** El motor registra estos archivos con
  `origin = 'host_storage_ref'` (valor nuevo; no hay columna ni migración) y
  `storage_key` igual a la llave que mandó ADP. El objeto es de ADP, no del motor. Si
  ADP lee esa tabla (lectura directa), debe tratar ese valor de `origin` como «objeto
  ajeno al motor». La clase de una fila (referencia del host o copia del motor) no
  cambia nunca: si el mismo `id` se vuelve a registrar en la otra clase para el mismo
  proveedor, el registro conserva la fila tal cual y el motor lo avisa.
- **`attachment_gc`.** Para una fila `host_storage_ref` el GC borra solo la fila y
  **nunca** llama al borrado de almacenamiento. Juzga y borra fila por fila (clave
  `sesión + documento + proveedor`): un documento con una fila del host bajo un
  proveedor y una copia del motor bajo otro ya no pierde la otra fila ni deja huérfana
  su copia. El contador `total_host_references_released` sube solo después de que la
  fila se borró, aparece en `gc.batch.end`, y el modo `--dry-run` dice «would release
  (drop the row, object kept)» sin imprimir la llave.
- **Por qué no hay otra defensa.** Probé dos defensas sin cambio de esquema y las
  descarté: (1) dejar `last_used_at` en el futuro para que un GC viejo no seleccione
  la fila: cualquier lectura la actualiza a «ahora» y vuelve a ser vencida; (2)
  guardar la llave con un prefijo (`host-ref:...`) para que un borrado viejo no
  alcance el objeto real: cambia lo que ADP lee de `conversation_attachments`. Por eso
  la garantía es el orden de despliegue de arriba. Si se quisiera forzarlo por
  configuración, una variable «el GC nuevo ya está desplegado» que el motor exigiera
  antes de registrar referencias del host sería la opción (no incluida: decisión del
  dueño).
- **`load_attachment` sobre uno de estos archivos.** El modelo recibe, como resultado
  de herramienta, `{"error":"large_tabular_file","document_id":…,"reason":"this file
  is too large to be read by this tool, and the large-file analysis tool is not
  available yet, so it cannot be analysed in this turn"}` y no se lee el objeto. El
  texto con la herramienta futura («use `attachment_run_python` with `tables`») queda
  detrás de `LARGE_FILE_TOOL_AVAILABLE`. El rechazo depende de la fila, no del interruptor: tampoco se lee entero tras un
  rollback. Para cualquier otra fila nada cambia (mismas consultas, errores y códigos).
- **Confianza en la llave (requisito para ADP).** El motor toma `storage_key` tal
  cual lo manda ADP y luego lo sirve (lecturas por flujo, URL de lectura). ADP
  **debe** verificar, antes de enviar la entrada, que la llave pertenece a la sesión
  y es un adjunto de chat. El motor solo hace comprobaciones baratas sin conocer el
  esquema de ADP: no vacía, hasta 1024 caracteres, sin caracteres de control y sin
  segmentos `..`; una entrada con una llave así se omite con un aviso.
