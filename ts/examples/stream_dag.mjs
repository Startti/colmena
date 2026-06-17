import { streamDag } from "colmena-ai";

const stream = await streamDag("tests/graphs/basic/power.json");
for await (const event of stream) {
  console.log(event.type, JSON.stringify(event));
}
