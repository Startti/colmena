# CHANGELOG — 2026-06

## 2026-06-05

### Bugfixes

- **`dag_engine`**: el engine deja de inyectar `__colmena_resume_answer` en nodos
  que no estaban suspendidos en el snapshot persistido. Arregla el error
  `llm_call resume: no pending tool call found in conversation history` cuando un
  `llm_call` está aguas abajo de un `suspend`, y también la cascada
  `suspend → suspend` que fallaba con `missing answer`. Sin cambio de API
  pública. Spec:
  [`docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`](superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md).
- **`llm_call`**: guard defensivo en la rama de resume. Si la rama recibe
  `__colmena_resume_answer` pero no hay un tool call pendiente en el historial,
  loggea `warn!` y cae a fresh run en vez de errorear. Spec §4.2.1.
