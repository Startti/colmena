"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.readSheetAsDataFrame = readSheetAsDataFrame;
exports.dataFrameToSheet = dataFrameToSheet;
const nodejs_polars_1 = require("nodejs-polars");
/** Parse an A1 address like "B12" into 0-based [col, row]. */
function parseAddr(addr) {
    const m = /^([A-Z]+)(\d+)$/.exec(addr);
    if (!m)
        throw new Error(`bad cell address: ${addr}`);
    let col = 0;
    for (const ch of m[1])
        col = col * 26 + (ch.charCodeAt(0) - 64);
    return [col - 1, parseInt(m[2], 10) - 1];
}
/** Convert a colmena-ai cell map (row 1 = headers, row 2+ = data) into a polars DataFrame. */
function readSheetAsDataFrame(cells) {
    const headers = [];
    const grid = [];
    for (const [addr, value] of Object.entries(cells)) {
        const [col, row] = parseAddr(addr);
        if (row === 0) {
            headers[col] = String(value);
        }
        else {
            (grid[row - 1] ??= [])[col] = value;
        }
    }
    const series = headers.map((name, col) => (0, nodejs_polars_1.Series)(name, grid.map((r) => (r ? (r[col] ?? null) : null))));
    return (0, nodejs_polars_1.DataFrame)(series);
}
/** Convert a polars DataFrame into the (columns, rows) shape colmena-ai's writeSheet expects. */
function dataFrameToSheet(df) {
    const columns = df.columns;
    const rows = df.toRecords().map((rec) => columns.map((c) => rec[c]));
    return { columns, rows };
}
