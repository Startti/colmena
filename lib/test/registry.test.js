"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const node_test_1 = require("node:test");
const strict_1 = __importDefault(require("node:assert/strict"));
const index_1 = require("../index");
(0, node_test_1.test)("defaultRegistry lists node types", () => {
    const registry = (0, index_1.defaultRegistry)();
    const types = registry.nodeTypes();
    strict_1.default.ok(Array.isArray(types));
    strict_1.default.ok(types.includes("log"), "expected the 'log' node type");
});
(0, node_test_1.test)("toolkitCatalog returns sub-tools for a toolkit node", () => {
    const registry = (0, index_1.defaultRegistry)();
    const types = registry.nodeTypes();
    if (!types.includes("api_explorer"))
        return; // env-gated
    const catalog = registry.toolkitCatalog("api_explorer", {});
    strict_1.default.ok(Array.isArray(catalog));
});
