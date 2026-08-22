#!/usr/bin/env python3
"""Renderiza un stream SSE de Colmena como árbol de anidación.

Dos vistas:
  1. ÁRBOL       — la jerarquía que arma `path`: quién corre dentro de quién.
  2. CRONOLÓGICO — el stream tal como llegó, indentado por `level`, para ver
                   el wrap en acción.

Uso:
    python3 scripts/render_nested_sse.py <archivo.sse> [--full] [--tree|--stream]

`--full` incluye los nodos de plomería (input/output). Por defecto se ocultan.
"""
import json
import sys

import os

# Respeta la convención NO_COLOR (https://no-color.org) y el flag --no-color,
# para poder pegar la salida en un doc o un ticket sin códigos de escape.
_PLAIN = bool(os.environ.get("NO_COLOR")) or "--no-color" in sys.argv

R = "" if _PLAIN else "\033[0m"
DIM = "" if _PLAIN else "\033[2m"
B = "" if _PLAIN else "\033[1m"
GREY = "" if _PLAIN else "\033[38;5;244m"
FG = "" if _PLAIN else "\033[38;5;253m"
LEVELS = [""] * 8 if _PLAIN else [
    "\033[38;5;39m", "\033[38;5;42m", "\033[38;5;214m", "\033[38;5;170m",
    "\033[38;5;203m", "\033[38;5;80m", "\033[38;5;227m", "\033[38;5;141m"]
PLUMBING = {"input", "output"}


def col(lv):
    return LEVELS[lv % len(LEVELS)]


def load(path):
    out = []
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.strip()
        if not line.startswith("data: "):
            continue
        payload = line[6:]
        if payload == "[DONE]":
            continue
        try:
            out.append(json.loads(payload))
        except json.JSONDecodeError:
            pass
    return out


def clip(s, n):
    s = " ".join(str(s).split())
    return s if len(s) <= n else s[: max(1, n - 1)] + "…"


def collect(frames, full):
    """path -> {level, kind, events[], text, think}, en orden de aparición."""
    nodes = {}
    order = []
    for f in frames:
        p = f.get("path")
        if not p:
            continue
        if p not in nodes:
            nodes[p] = {"level": f.get("level", 0), "kind": None,
                        "events": [], "text": "", "think": ""}
            order.append(p)
        n = nodes[p]
        t = f.get("type", "")

        if t.endswith("text-delta"):
            n["text"] += f.get("delta", "")
        elif t == "thinking-delta":
            n["think"] += f.get("delta", "")
        elif t.endswith("node-start"):
            n["kind"] = f.get("node_type") or n["kind"]
        elif t == "agent-turn" and not n["kind"]:
            # Un `llm_call` despachado como tool no emite `node-start` propio;
            # su turno de mensaje es la única señal de que ahí corre un agente.
            n["kind"] = "llm_call"
        elif t.endswith("tool-input-available"):
            n["events"].append(("call", f.get("toolName", "?"),
                                json.dumps(f.get("input"), ensure_ascii=False)))
        elif t.endswith("tool-output-available"):
            n["events"].append(("ret", "", json.dumps(f.get("output"), ensure_ascii=False)))
        elif t.endswith("batch-item-finished"):
            n["events"].append(("row", f"fila {f.get('index')}", f.get("status", "")))

    if not full:
        for p in list(nodes):
            if nodes[p]["kind"] in PLUMBING and not nodes[p]["events"] \
                    and not nodes[p]["text"]:
                del nodes[p]
                order.remove(p)
    return nodes, order


def build_tree(paths):
    """Trie de segmentos: {segmento: (path_completo, hijos)}."""
    root = {}
    for p in paths:
        cur = root
        for i, seg in enumerate(p.split(">")):
            full = ">".join(p.split(">")[: i + 1])
            cur = cur.setdefault(seg, {"__path__": full, "__kids__": {}})["__kids__"]
    return root


KIND_ICON = {"llm_call": "◆", "subgraph": "▣", "orchestrator": "⬢",
             "python_script": "⚙", "for_each": "⋔"}


def render_tree(nodes, order, full, width=104):
    tree = build_tree(order)
    print(f"\n {B}ÁRBOL DE EJECUCIÓN{R}  {GREY}— quién corre dentro de quién, "
          f"según el campo `path`{R}\n")

    def walk(kids, prefix):
        items = sorted(kids.items(), key=lambda kv: order.index(kv[1]["__path__"])
                       if kv[1]["__path__"] in order else 10**6)
        for i, (seg, meta) in enumerate(items):
            last = i == len(items) - 1
            elbow = "└─" if last else "├─"
            cont = "   " if last else "│  "
            n = nodes.get(meta["__path__"])
            lv = n["level"] if n else 0
            c = col(lv)
            icon = KIND_ICON.get(n["kind"] if n else None, "·")
            label = f"{c}{elbow} {icon} {B}{seg}{R}"
            badge = f"{GREY}L{lv}{R}" if n else ""
            print(f" {prefix}{label}  {badge}")

            if n:
                inner = prefix + cont
                for kind, name, detail in n["events"]:
                    room = width - len(prefix) - 22
                    if kind == "call":
                        print(f" {inner}{c}│{R}   {GREY}↳ tool{R} {FG}{name}{R} "
                              f"{GREY}{clip(detail, room)}{R}")
                    elif kind == "ret":
                        print(f" {inner}{c}│{R}   {GREY}↲ →{R} {GREY}{clip(detail, room)}{R}")
                    else:
                        print(f" {inner}{c}│{R}   {GREY}⋯ {name} {detail}{R}")
                if n["think"]:
                    print(f" {inner}{c}│{R}   {DIM}✳ {clip(n['think'], width - len(prefix) - 18)}{R}")
                if n["text"]:
                    print(f" {inner}{c}│{R}   {FG}“{clip(n['text'], width - len(prefix) - 18)}”{R}")
            walk(meta["__kids__"], prefix + cont)

    walk(tree, "")
    print()


def render_stream(frames, width=104):
    print(f" {B}STREAM EN ORDEN DE LLEGADA{R}  {GREY}— indentado por `level`; "
          f"así se ve el wrap{R}\n")
    buf = {}

    def flush(p):
        b = buf.pop(p, None)
        if not b or not b["text"].strip():
            return
        lv = b["level"]
        pad = "  " * lv
        print(f" {col(lv)}{'│ ' * lv}{R}{FG}“{clip(b['text'], width - len(pad) - 8)}”{R}")

    for f in frames:
        t = f.get("type", "")
        lv = f.get("level", 0)
        p = f.get("path", "")
        c = col(lv)
        guide = f"{c}{'│ ' * lv}{R}"

        if t.endswith("text-delta") or t == "thinking-delta":
            buf.setdefault(p, {"level": lv, "text": ""})["text"] += f.get("delta", "")
            continue
        if t.endswith("text-end"):
            flush(p)
            continue

        if t.endswith("node-start") and f.get("node_type") not in PLUMBING:
            icon = KIND_ICON.get(f.get("node_type"), "·")
            flush(p)
            print(f" {guide}{c}┌ {icon} {B}{f.get('node_id')}{R} "
                  f"{GREY}{f.get('node_type')}{R}")
        elif t.endswith("tool-input-available"):
            flush(p)
            print(f" {guide}{c}│{R} {GREY}↳ tool{R} {FG}{f.get('toolName')}{R} "
                  f"{GREY}{clip(json.dumps(f.get('input'), ensure_ascii=False), 44)}{R}")
        elif t.endswith("tool-output-available"):
            flush(p)
            print(f" {guide}{c}│{R} {GREY}↲ → {clip(json.dumps(f.get('output'), ensure_ascii=False), 52)}{R}")
        elif t.endswith("node-end") and f.get("node_type") not in PLUMBING:
            flush(p)
            print(f" {guide}{c}└ {GREY}{f.get('node_id')} listo{R}")
    for p in list(buf):
        flush(p)
    print()


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    full = "--full" in sys.argv
    only_tree = "--tree" in sys.argv
    only_stream = "--stream" in sys.argv
    frames = load(args[0])
    lv = sorted({f.get("level", 0) for f in frames})
    print(f"\n {GREY}{len(frames)} frames · niveles {min(lv)}–{max(lv)} · {args[0]}{R}")
    nodes, order = collect(frames, full)
    if not only_stream:
        render_tree(nodes, order, full)
    if not only_tree:
        render_stream(frames)


if __name__ == "__main__":
    main()
