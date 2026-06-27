---
name: anatomia-de-un-buen-specs
description: Use cuando dudes qué debe contener cada una de las 8 secciones del documento de specs, o si una sección ya está completa o sigue vaga.
---

# Anatomía de un buen documento de specs

El documento tiene SIEMPRE estas 8 secciones, con estos títulos exactos. Para cada una,
esto es lo que la hace "completa" (vs vaga):

1. **Objetivo** — el problema de negocio en 1-3 frases. Bien: "Cada vez que llega un correo
   de un cliente con una orden, registrar la orden en la planilla de ventas y avisar al
   vendedor." Vago: "Automatizar correos."

2. **Disparador / trigger** — qué inicia la tarea y con qué frecuencia. ¿Un correo que llega?
   ¿Una hora del día? ¿Que alguien llena un formulario? Bien: "Llega un correo a ventas@ con
   asunto que empieza por 'Orden'." Vago: "Cuando haga falta."

3. **Actores y sistemas** — qué cuentas, apps y servicios participan (Gmail, un CRM concreto,
   una planilla concreta, una API). Nombrá cada uno. Bien: "Gmail (ventas@), Google Sheet
   'Ventas 2026', CRM HubSpot." Vago: "El correo y el Excel."

4. **Procedimiento paso a paso** — la narración COMPLETA de lo que hoy hace la persona a
   mano, en orden. Cada paso, una acción. Si hay decisiones ("si el correo no trae adjunto…"),
   anotalas. Es la sección más importante.

5. **Datos** — qué información ENTRA, qué SALE, y qué campos. Bien: "Entra: nombre, email,
   producto, cantidad. Sale: una fila nueva en la planilla con esas columnas + fecha."

6. **Conexiones y credenciales** — qué accesos harán falta para que esto funcione (login a
   Gmail, token del CRM, permiso de edición de la planilla). NO pidas las claves acá; solo
   listá QUÉ accesos se necesitan. Esta sección le dice al constructor qué credenciales pedir.

7. **Criterios de éxito** — cómo sabremos, de forma chequeable, que quedó bien. Bien: "La fila
   aparece en la planilla con los datos correctos y el vendedor recibe el aviso en < 5 min."

8. **Casos borde y errores** — qué hacer cuando algo no sale como lo normal: el correo no trae
   adjunto, el dato viene incompleto, el sistema externo falla. Listá los que la persona
   mencione o que sean obvios para esta tarea.

## Definición de "terminado"

El documento está listo SOLO si las 8 secciones están presentes y ninguna está vacía ni es
genérica. Si una sección quedó vaga, volvé a preguntar por ESA sección antes de entregar.
