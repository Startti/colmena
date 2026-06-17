import { runDag } from "colmena-ai";

const result = await runDag("tests/graphs/basic/power.json");
console.log("DAG output:", JSON.stringify(result, null, 2));
