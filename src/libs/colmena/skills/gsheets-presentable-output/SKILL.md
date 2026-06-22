---
name: gsheets-presentable-output
description: Use when calling gsheets_format_range to make a sheet presentable. Covers the data→formulas→format order, ready-to-use color palettes, number-format patterns (currency/%/date), a full multi-op template, and layout rules. Load the reference for your scenario.
references:
  - name: recipe
    description: Step-by-step recipe for a professional report. The correct order is data → formulas → format (format LAST, over the populated range). Read this first.
  - name: palettes
    description: Ready-to-use hex palettes that look good — header blue/dark-gray, white header text, subtle zebra, light-gray totals row. Copy these instead of inventing colors.
  - name: number_formats
    description: numberFormat patterns — currency ($#,##0), percent (0.0%), date, thousands. When to use each, with exact pattern strings.
  - name: multi_op_template
    description: A COMPLETE ops:[...] JSON for a typical report (title, header, currency on amounts, totals row, table borders, column widths). Copy and adapt the ranges.
  - name: layout
    description: Layout rules — text left / numbers right, column widths, table borders, separating the totals row, optional zebra striping.
---

# gsheets — Presentable output best practices

Quick rules:

1. **Format is separate from values.** Write data + formulas first with
   `gsheets_set_range` / `gsheets_set_cell` (formulas start with `=`), THEN
   style with `gsheets_format_range`. Never expect format to change values.
2. **One multi-op call.** Send all the formatting as a single
   `gsheets_format_range` with several `ops` — header, currency, borders,
   totals row, widths — not one call per attribute.
3. **Always make human-facing output presentable by default**, even when the
   user did not ask for formatting explicitly. A bare grid of numbers is not
   an acceptable deliverable.

Load the reference that matches your task: start with `recipe` for the full
flow, `multi_op_template` to copy a ready ops payload, `number_formats` for the
exact pattern strings, `palettes` for colors, `layout` for alignment/widths.
