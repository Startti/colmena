# Entradas de `files[]` con solo `storage_key` (archivos tabulares grandes)

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
  log (WARN, siempre activo) el motivo concreto, con el nombre del archivo como
  texto inerte y nunca la llave (hasta 10 avisos por turno); el aviso al modelo llega
  en el cambio siguiente.
- Si la entrada trae además `data`, `url` o `path`, se interpreta como hasta ahora
  (prioridad `data > url > path`): la llave solo se usa cuando no hay ninguna otra
  fuente. El emisor debe omitir `data` y `url` para los archivos grandes.
- Rust: `llm::domain::FileSource` tiene una variante nueva, `StorageRef(String)`. Un
  `match` exhaustivo sobre ese tipo deja de compilar. Comprobado en el repo de ADP
  (no en este): `git grep FileSource -- apps` no devuelve ningún resultado, así que
  no hay `match` que actualizar.
