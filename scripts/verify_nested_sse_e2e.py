#!/usr/bin/env python3
"""Verifica el SSE de tests/graphs/advanced/nested_sse_remediation_e2e.json.

Un solo run del grafo cubre los siete defectos de la remediación de anidación
del 2026-08-21 más la eliminación del tope de profundidad. Este script parsea el
stream y afirma cada uno por separado, para que un fallo diga CUÁL se rompió.

Uso:
    python3 scripts/verify_nested_sse_e2e.py <archivo.sse> [--ceiling]

`--ceiling` cambia las expectativas al modo "techo configurado"
(COLMENA_MAX_SUBGRAPH_DEPTH activo): ahí la cadena profunda DEBE ser rechazada.
"""
import json
import sys

EXPECTED_DEPTH = 7  # 6 subgrafos anidados + el subgraph-as-tool que los envuelve


def load(path):
    frames = []
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.strip()
        if not line.startswith("data: "):
            continue
        try:
            frames.append(json.loads(line[6:]))
        except json.JSONDecodeError:
            pass
    return frames


class Report:
    def __init__(self):
        self.rows = []

    def check(self, name, ok, detail):
        self.rows.append((name, bool(ok), detail))

    def render(self):
        width = max(len(n) for n, _, _ in self.rows)
        failed = 0
        for name, ok, detail in self.rows:
            mark = "PASS" if ok else "FAIL"
            if not ok:
                failed += 1
            print(f"  [{mark}] {name.ljust(width)}  {detail}")
        return failed


def paths_under(frames, prefix):
    return {f.get("path") for f in frames
            if isinstance(f.get("path"), str) and f["path"].startswith(prefix)}


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    ceiling_mode = "--ceiling" in sys.argv
    frames = load(args[0])
    r = Report()

    types = [f.get("type") for f in frames]
    blob = json.dumps(frames, ensure_ascii=False)

    # ── Tope de profundidad ──────────────────────────────────────────────────
    rejected = "SUBGRAPH_DEPTH_EXCEEDED" in blob
    if ceiling_mode:
        r.check("techo configurado rechaza y con código estable", rejected,
                "se encontró SUBGRAPH_DEPTH_EXCEEDED" if rejected
                else "NO se encontró el código de error")
    else:
        reached = f'"depth_reached\\":{EXPECTED_DEPTH}' in blob or \
                  f'"depth_reached": {EXPECTED_DEPTH}' in blob or \
                  f'depth_reached\\":{EXPECTED_DEPTH}' in blob
        r.check("profundidad sin tope (antes se rechazaba en 5)",
                reached and not rejected,
                f"depth_reached={EXPECTED_DEPTH} alcanzado, sin rechazo" if reached and not rejected
                else f"no se observó depth_reached={EXPECTED_DEPTH} (rechazado={rejected})")
        r.check("contador de profundidad sobrevive cada frontera (#6)", reached,
                "el nodo más profundo reportó su nivel" if reached
                else "el contador no llegó al fondo")

    # ── #1 frontera en la ruta tool ──────────────────────────────────────────
    boundaries = {f.get("node_id") for f in frames
                  if f.get("type") in ("subgraph-node-start", "subgraph-node-end")
                  and f.get("node_type") == "subgraph"}
    tool_boundaries = boundaries & {"cadena_profunda", "experto_directo", "abanico"}
    r.check("#1 subgraph-as-tool emite frontera",
            "cadena_profunda" in boundaries,
            f"fronteras de subgraph observadas: {sorted(tool_boundaries) or 'NINGUNA'}")

    # ── Todo tool que corre trabajo interno delimita su sub-árbol ────────────
    inner_work = {"experto_directo": "llm_call", "abanico": "for_each"}
    all_bounds = {f.get("node_id") for f in frames
                  if f.get("type") in ("subgraph-node-start", "subgraph-node-end")}
    missing = [t for t in inner_work if t not in all_bounds]
    r.check("frontera también para llm_call/for_each como tool",
            not missing,
            f"{sorted(inner_work)} delimitados" if not missing
            else f"sin frontera: {missing}")

    # ── #7 el contenido anida BAJO su frontera ───────────────────────────────
    under = paths_under(frames, "coordinador>cadena_profunda>")
    r.check("#7 contenido anidado bajo la frontera (no al lado)",
            bool(under),
            f"{len(under)} path(s) bajo 'coordinador>cadena_profunda>'" if under
            else "ningún path contiene la frontera como segmento")

    # ── #3 llm_call-as-tool es un nivel propio ───────────────────────────────
    exp = [f for f in frames
           if f.get("path", "").startswith("coordinador>experto_directo")
           and f.get("level", 0) >= 1]
    r.check("#3 llm_call-as-tool con nivel e identidad propios",
            bool(exp),
            f"{len(exp)} frame(s) en level>=1 bajo 'coordinador>experto_directo'" if exp
            else "sus eventos siguen atribuidos al padre en level 0")

    # ── #2 el thinking del planner comparte nivel y path con su node-start ───
    starts = {(f.get("node_id")): (f.get("level"), f.get("path"))
              for f in frames if f.get("type") == "subgraph-node-start"}
    think = [f for f in frames if f.get("type") == "thinking-delta"]
    mismatched = [f for f in think
                  if f.get("node_id") in starts
                  and (f.get("level"), f.get("path")) != starts[f["node_id"]]]
    ids = sorted({f.get("node_id") for f in think})
    if think:
        r.check("#2 thinking al mismo nivel/path que su node-start",
                not mismatched,
                f"{len(think)} thinking-delta de {ids}, 0 desalineados" if not mismatched
                else f"{len(mismatched)} desalineados: {mismatched[0]}")
    else:
        r.check("#2 thinking al mismo nivel/path que su node-start", False,
                "no se emitió ningún thinking-delta (¿planner sin streaming:true?)")
    r.check("#2 el thinking NO se reclasifica como texto del agente",
            not any(f.get("type") == "subgraph-thinking-delta" for f in frames),
            "sigue siendo 'thinking-delta'; no existe un tipo subgraph-thinking-delta")

    # ── #4 bloques de texto por rama ─────────────────────────────────────────
    opened, closed, doubles = {}, set(), []
    for f in frames:
        t, fid, path = f.get("type"), f.get("id"), f.get("path")
        if t in ("text-start", "subgraph-text-start"):
            if fid in opened:
                doubles.append(fid)
            opened[fid] = path
        elif t in ("text-end", "subgraph-text-end"):
            closed.add(fid)
    leaked = set(opened) - closed
    r.check("#4 bloques de texto sin colisión ni fuga",
            not doubles and not leaked,
            f"{len(opened)} abiertos / {len(closed)} cerrados, 0 reusados, 0 sin cerrar"
            if not doubles and not leaked
            else f"reusados={doubles} sin_cerrar={len(leaked)}")

    # Las filas concurrentes del for_each comparten node_id: sus bloques deben
    # ser distintos. Es el escenario exacto del defecto.
    fan = {fid: p for fid, p in opened.items() if p and ">eco" in p}
    r.check("#4 filas concurrentes con bloques independientes",
            len(set(fan.values())) == len(fan),
            f"{len(fan)} bloque(s) del fan-out, todos con path distinto"
            if len(set(fan.values())) == len(fan) else f"paths repetidos: {fan}")

    # ── Los eventos propios de un for_each-as-tool viven DENTRO del tool ─────
    batch = [f for f in frames if "batch" in f.get("type", "")]
    stray = [f for f in batch if not f.get("path", "").startswith("coordinador>abanico")]
    if batch:
        r.check("batch de un for_each-as-tool anidado bajo su tool",
                not stray,
                f"{len(batch)} frame(s) de batch, todos bajo 'coordinador>abanico'"
                if not stray else f"{len(stray)} fuera del tool, p.ej. {stray[0].get('path')}")

    print(f"\n  frames: {len(frames)}  |  tipos distintos: {len(set(types))}")
    print(f"  niveles observados: {sorted({f.get('level') for f in frames})}\n")
    failed = r.render()
    print()
    if failed:
        print(f"  {failed} verificación(es) FALLARON")
        return 1
    print("  todas las verificaciones pasaron")
    return 0


if __name__ == "__main__":
    sys.exit(main())
