# `usage` ahora separa input fresco de tokens de cache

**Acción de ADP: OBLIGATORIA si se factura sobre `usage`.** Ningún campo cambió
de nombre. Dos campos cambiaron de **valor**, y dos que antes podían faltar ahora
están siempre.

## Qué pasaba

`promptTokens` significaba tres cosas distintas según el provider — Anthropic lo
reportaba neto de cache, OpenAI y Gemini con el cache adentro — y el motor las
sumaba en el mismo acumulador como si fueran comparables. La tabla completa por
provider vive en
[§14](../developer_guide/14_llm_deep_dive.md#semántica-de-prompttokens-normalizada-sin-restas-del-consumidor).

De ahí salían dos consecuencias con plata de por medio:

1. **`totalTokens` subcontaba los turnos cacheados de Anthropic un 81%.**
   Medición real: `prompt_tokens: 404`, `cache_read_tokens: 1809`,
   `total_tokens: 412` — el turno procesó 2213 tokens de entrada.
2. **La fórmula de costo que documentaba la guía §14 daba negativo en Anthropic**
   (`404 − 1809 = −1405`). Vivía en la documentación de colmena, **no** en el
   código de ADP — ver la sección de acciones.

Además `cacheReadTokens` / `cacheWriteTokens` solo aparecían si eran `> 0`, así
que un campo ausente no se distinguía de un provider que no reporta el dato —
dos situaciones con implicaciones de costo opuestas.

## Qué cambia

**Nombres: ninguno.** `promptTokens`, `completionTokens`, `totalTokens`,
`cacheReadTokens`, `cacheWriteTokens`, `thinkingTokens` y sus equivalentes en
snake_case por nodo siguen escribiéndose igual.

| Campo | Antes | Ahora |
|---|---|---|
| `promptTokens` | Anthropic: fresco. OpenAI/Gemini: fresco + cache | **Fresco en los tres.** Los adapters normalizan |
| `totalTokens` | `prompt + completion + thinking` | `prompt + completion + thinking + cacheRead + cacheWrite` |
| `cacheReadTokens` | Solo si `> 0` | **Siempre presente**, `0` incluido |
| `cacheWriteTokens` | Solo si `> 0` | **Siempre presente**, `0` incluido |
| `thinkingTokens` | Solo si `> 0` | Sin cambios — sigue con gate `> 0` |

El `finish` de un run **cancelado** ahora emite el mismo objeto que un run
terminado. Antes perdía las columnas de cache y thinking por completo.

Se sostiene en los tres providers que
`promptTokens + cacheReadTokens + cacheWriteTokens` es el input real del turno,
por lo que el agregado run-level vuelve a ser sumable aunque un grafo mezcle
providers.

## Qué tiene que hacer ADP

Esta sección se reescribió el 2026-08-24 tras **leer el código de facturación de
ADP** en vez de suponerlo. La versión anterior advertía sobre una resta
`promptTokens − cacheReadTokens` que produciría costos negativos: **esa resta no
existe en ADP**, y la advertencia era falsa. El riesgo real es el opuesto.

El cálculo vigente vive en
`apps/api/src/billing/application/usage-event.service.ts`:

```js
const rawInputTokens  = u.promptTokens ?? 0;
const rawOutputTokens = (u.completionTokens ?? 0) + (u.thinkingTokens ?? 0);
computeStCost({ rawInputTokens, rawOutputTokens, inputUsdPerMillion, outputUsdPerMillion, ... });
```

Tres hechos que se desprenden de ahí:

- ADP **no** resta el cache en ninguna parte.
- ADP **no** factura sobre `totalTokens` (solo lo propaga a los reportes de uso).
- ADP **ignora las dos columnas de cache**: no llegan al `tokenUsage` que
  alimenta el cálculo.

**1. ACCIÓN REQUERIDA — los tokens de cache dejaron de facturarse.** En Gemini y
OpenAI los tokens cacheados salieron de `promptTokens`, y como el cálculo no mira
`cacheReadTokens`, ahora se cobran a **cero**:

| Provider | Antes | Ahora | Efecto |
|---|---|---|---|
| Gemini / OpenAI | cache dentro de `promptTokens`, a tarifa completa | cache fuera y las columnas ignoradas | sobrecobro → **subcobro** |
| Anthropic | ya venía neto de cache | igual | sin cambio |

Un cache read cuesta entre el **10% y el 50%** de la tarifa de input según el
provider y el modelo. No es gratis. El arreglo es sumar las dos columnas al
cálculo, cada una a su tarifa:

```
costo_input = promptTokens     × rate_input
            + cacheReadTokens  × rate_input × f_read     (0.10 Anthropic/gpt-5.x · 0.50 gpt-4o · 0.10 Gemini 2.5+)
            + cacheWriteTokens × rate_input × 1.25       (solo Anthropic y OpenAI GPT-5.6+; 0 en el resto)
costo_output = (completionTokens + thinkingTokens) × rate_output
```

**2. `totalTokens` cambia de magnitud, pero no afecta la factura.** Sube en todo
turno cacheado — no porque se gaste más, sino porque antes omitía tokens que el
provider sí cobraba. Como la facturación no lo usa, el impacto está solo en los
reportes de uso (`usage-report.service.ts`), donde los totales mostrados van a
saltar.

**3. Nada que hacer para los campos siempre-presentes.** Un consumidor que ya
usaba `?? 0` sigue funcionando sin tocar nada.

## Por qué read y write siguen separados

Un cache read cuesta ~10% de la tarifa de input y un write ~125%: más de 10x de
diferencia, así que un campo único de "cache" no sería facturable. Detalle por
provider en [§14](../developer_guide/14_llm_deep_dive.md).

## Cómo verificarlo

```bash
set -a; source .env; set +a; unset ANTHROPIC_BASE_URL; unset COLMENA_LOCAL
S=verif_$(date +%s)
for i in 1 2; do
  cargo run --bin dag_engine -- run \
    tests/graphs/agents/provider_cache_temporal_anthropic_e2e.json \
    --agent-session-id ${S}_$i --include-extra-info \
  | grep -o '"usage":{[^}]*}' | tail -1
done
```

El segundo run debe mostrar `cache_read_tokens > 0`, `prompt_tokens` bajo (el
input fresco, no el prefijo entero) y un `total_tokens` que sea la suma de todas
las columnas.
