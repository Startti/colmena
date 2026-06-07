#!/usr/bin/env python3
"""Generate q3.xlsx and q4.xlsx for the F browser smoke.

Schema: Producto | Cantidad | Precio | Total (Total left empty so the
import path matches the C smoke's behaviour, with a title row in A1).

Overlap design (deterministic for reproducible smokes):
- 10 SKUs in common (SKU-0001 .. SKU-0010)
- 3 SKUs only in Q3 (SKU-Q3-only-1..3)
- 3 SKUs only in Q4 (SKU-Q4-only-1..3)
- 2 of the common SKUs have a different Precio in Q4 (drift)
"""
import random
import pandas as pd
from pathlib import Path

random.seed(2026)
OUT = Path('/tmp/colmena_e2e')
OUT.mkdir(parents=True, exist_ok=True)


def make(period: str, only_skus: list[str], price_overrides: dict[str, float]) -> None:
    rows = [
        ['Reporte ' + period + ' 2026', '', '', ''],
        ['Producto', 'Cantidad', 'Precio', 'Total'],
    ]
    common = [f'SKU-{i:04d}' for i in range(1, 11)]
    all_skus = common + only_skus
    for sku in all_skus:
        qty = random.choice([1, 2, 3, 5, 8, 12])
        if sku in price_overrides:
            price = price_overrides[sku]
        else:
            price = round(random.uniform(5.0, 200.0), 2)
        rows.append([sku, qty, price, ''])
    pd.DataFrame(rows).to_excel(OUT / f'{period.lower()}.xlsx', index=False, header=False)
    print(f'wrote {OUT / (period.lower() + ".xlsx")} ({len(rows)} rows)')


make('Q3', only_skus=['SKU-Q3-ONLY-1', 'SKU-Q3-ONLY-2', 'SKU-Q3-ONLY-3'], price_overrides={})
make('Q4', only_skus=['SKU-Q4-ONLY-1', 'SKU-Q4-ONLY-2', 'SKU-Q4-ONLY-3'],
     price_overrides={'SKU-0003': 999.99, 'SKU-0007': 0.99})
