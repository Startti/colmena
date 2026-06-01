# Spike GO/NO-GO checklist

> Run when implementation tasks 1–11 are complete. Each item is a
> single check; record the outcome inline before moving on.

## R1.1 — Convergence multi-peer (automated)

```bash
cargo test --test spike_convergence_test -- --nocapture
```

- [ ] PASS

## R1.1 manual — two browser tabs + spike-agent

```bash
cargo run --bin dag_engine -- spike-yws --port 8081 &
open "http://127.0.0.1:8081/?artifact=r1man"
open "http://127.0.0.1:8081/?artifact=r1man"   # second tab
```

In one tab, type a value in A1. Observe the other tab updates within 1s.

Then:

```bash
cargo run --bin dag_engine -- spike-agent ws \
  --url ws://127.0.0.1:8081/yjs/r1man --sheet s1 --addr A2 --value "from-agent"
```

Observe both tabs reflect A2 = "from-agent" within 1s.

- [ ] PASS — record observed latency: ____ ms
- [ ] FAIL — describe symptom:

## R1.2 — Univer works without its own collab backend

Confirm via DevTools Network tab during the R1.1 manual test:
- Only outbound WS frames are to `ws://127.0.0.1:8081/yjs/<artifact>`.
- No requests to any `@univerjs/collaboration-server` endpoint.
- The page successfully renders and accepts edits.

- [ ] PASS
- [ ] FAIL — note what Univer demanded:

## R2.1 — Projection p50 < 50ms on 1000 cells

```bash
cargo test -p colmena_dag_engine --lib spike::projection::tests::r2_1_benchmark -- --ignored --nocapture
```

Expected line: `projection p50 = X.XXms, p95 = Y.YYms (1000 cells, 100 runs)`.

- [ ] PASS — p50 = ____ ms, p95 = ____ ms
- [ ] FAIL

## R2.2 — Projection LoC < 500

```bash
# Counts logic only (excludes tests + helpers).
awk '/^#\[cfg\(test\)\]/{exit} {print}' \
  src/libs/colmena/src/dag_engine/spike/projection.rs | wc -l
```

- [ ] PASS — ____ LoC
- [ ] FAIL

## R2.3 — Projection survives 50 random edits

```bash
# Manually: in one tab, paste varied values across 50 cells, then:
curl -s http://127.0.0.1:8081/projection/r1man.json | jq . > /dev/null && echo OK
```

- [ ] PASS — valid JSON
- [ ] FAIL

## R5.1 — `.xlsx` ingestion visual fidelity

Open `http://127.0.0.1:8081/?artifact=r5` then in DevTools:

```js
fetch('/spike.xlsx').then(r => r.blob()).then(b => {
  // Use Univer's built-in xlsx importer; the exact API call depends
  // on version. See `@univerjs/sheets-data-validation` README or
  // `FUniver` facade.
  // For the spike: drop the file into Univer's UI uploader if a
  // toolbar button exists, or use the imported `import { xlsx }`
  // helper if available.
});
```

Expected visual check:
- Header row colored.
- Title row merged across columns A–D.
- Formula cell `D3` shows the computed value (e.g. `=B3*C3` resolves).
- All 250 data rows visible.

- [ ] PASS — screenshots saved to `/tmp/spike/r5-screenshots/`
- [ ] FAIL — describe:

## R5.2 — Projection of imported xlsx captures values

After R5.1 succeeds, with the doc loaded:

```bash
curl -s http://127.0.0.1:8081/projection/r5.json | jq '.sheets[0].cells | length'
```

Expected: ≥ 1004 (4 header cells + 1000 data cells; the title cell may
be present or absent depending on whether the bridge copies merged
ranges).

Pick ten random cells from the fixture and verify their values match:

```bash
curl -s http://127.0.0.1:8081/projection/r5.json | jq '.sheets[0].cells.A3, .sheets[0].cells.B3, .sheets[0].cells.D3'
```

- [ ] PASS — ≥99% of non-formula values match
- [ ] FAIL — describe:

---

## Overall verdict

- R1.1: ___ / R1.2: ___ / R2.1: ___ / R2.2: ___ / R2.3: ___ / R5.1: ___ / R5.2: ___

- [ ] GO — all PASS. Proceed to v1 spec (separate brainstorm).
- [ ] NO-GO — at least one FAIL. Document hallazgos in results spec.
