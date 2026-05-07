"""Smoke test for the api_explorer toolkit node via PyO3 bindings.

Validates that:
  1. The petstore graph parses and validates without errors.
  2. The node registers under node_type="api_explorer".
  3. sub_tool_catalog surfaces the five expected sub-tools through the
     toolkit-node registry helper.

No live network; uses in-process registry inspection only.
"""

import json
import os

import colmena  # PyO3 binding


GRAPH_PATH = os.path.abspath(
    os.path.join(
        os.path.dirname(__file__),
        "..",
        "..",
        "tests",
        "graphs",
        "web",
        "api_explorer_petstore.json",
    )
)


def test_graph_loads_and_validates():
    with open(GRAPH_PATH, encoding="utf-8") as f:
        graph = json.load(f)
    colmena.validate_graph(graph)


def test_api_explorer_node_registered():
    registry = colmena.default_registry()
    node_types = registry.node_types()
    assert "api_explorer" in node_types, (
        f"api_explorer missing from registry; got {sorted(node_types)}"
    )


def test_api_explorer_catalog_has_five_sub_tools():
    registry = colmena.default_registry()
    catalog = registry.toolkit_catalog("api_explorer", {})
    names = sorted(entry["name"] for entry in catalog)
    assert names == [
        "build_http_request",
        "get_endpoint_details",
        "list_endpoints",
        "load_spec",
        "search_endpoint",
    ]
