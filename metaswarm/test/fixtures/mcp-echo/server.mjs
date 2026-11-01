// A tiny MCP server over stdio: newline-delimited JSON-RPC, no dependencies.
import { createInterface } from "node:readline";
import { writeFileSync } from "node:fs";
import { networkInterfaces } from "node:os";

const send = (msg) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...msg }) + "\n");
const obj = (properties = {}, required = []) => ({ type: "object", properties, required });
const tools = [
  { name: "echo", description: "Echo the text back.", inputSchema: obj({ text: { type: "string" } }, ["text"]) },
  { name: "add", description: "Add two numbers.", inputSchema: obj({ a: { type: "number" }, b: { type: "number" } }, ["a", "b"]) },
  { name: "fail", description: "Always fails.", inputSchema: obj() },
  { name: "mixed", description: "Text plus an image.", inputSchema: obj() },
  { name: "slow", description: "Answers after 10s.", inputSchema: obj() },
  { name: "sandbox", description: "Report the sandbox: network interfaces, writable root, env.", inputSchema: obj() },
];

async function call(name, args) {
  switch (name) {
    case "echo":
      return { content: [{ type: "text", text: `echo: ${args.text}` }] };
    case "add":
      return { content: [{ type: "text", text: String(args.a + args.b) }] };
    case "fail":
      return { isError: true, content: [{ type: "text", text: "it broke" }] };
    case "mixed":
      return { content: [{ type: "text", text: "see picture" }, { type: "image", data: "AAAA", mimeType: "image/png" }] };
    case "slow":
      await new Promise((r) => setTimeout(r, 10_000));
      return { content: [{ type: "text", text: "late" }] };
    case "sandbox": {
      let rootWritable = true;
      try {
        writeFileSync("/probe", "x");
      } catch {
        rootWritable = false;
      }
      const ifaces = Object.keys(networkInterfaces());
      return { content: [{ type: "text", text: JSON.stringify({ ifaces, rootWritable, secret: process.env.FIXTURE_SECRET ?? null }) }] };
    }
  }
  throw new Error(`no tool ${name}`);
}

createInterface({ input: process.stdin }).on("line", async (line) => {
  if (!line.trim()) return;
  const msg = JSON.parse(line);
  if (msg.id === undefined) return; // notifications
  try {
    if (msg.method === "initialize") {
      const info = { name: "echo-fixture", version: "1.0.0" };
      send({ id: msg.id, result: { protocolVersion: msg.params.protocolVersion, capabilities: { tools: {} }, serverInfo: info } });
    } else if (msg.method === "tools/list") send({ id: msg.id, result: { tools } });
    else if (msg.method === "tools/call") send({ id: msg.id, result: await call(msg.params.name, msg.params.arguments ?? {}) });
    else if (msg.method === "ping") send({ id: msg.id, result: {} });
    else send({ id: msg.id, error: { code: -32601, message: `no method ${msg.method}` } });
  } catch (e) {
    send({ id: msg.id, error: { code: -32603, message: String(e?.message ?? e) } });
  }
});
