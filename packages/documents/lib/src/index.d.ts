import { DataFrame } from "nodejs-polars";
type Cell = string | number | boolean | null;
type CellMap = Record<string, Cell>;
/** Convert a colmena-ai cell map (row 1 = headers, row 2+ = data) into a polars DataFrame. */
export declare function readSheetAsDataFrame(cells: CellMap): DataFrame;
/** Convert a polars DataFrame into the (columns, rows) shape colmena-ai's writeSheet expects. */
export declare function dataFrameToSheet(df: DataFrame): {
    columns: string[];
    rows: Cell[][];
};
export {};
