"""Type stubs for the ``colmena`` native extension (PyPI package ``colmena-ai``).

Covers the primary LLM and DAG-engine surface. The ``colmena.documents``
submodule (CRDT sheets) is documented in ``docs/developer_guide/`` rather than
stubbed here.
"""

from typing import Any, Awaitable, Dict, List, Optional, Union

class LlmException(Exception):
    """Raised by ``ColmenaLlm`` calls (``call`` / ``stream`` / ``health_check``)."""

class DagException(Exception):
    """Raised by the DAG functions (``run_dag`` / ``validate_graph`` / ``serve_dag``)."""

class LlmConfigOptions:
    """Model and sampling parameters for an LLM call.

    Any field left as ``None`` falls back to the provider default.
    """

    api_key: Optional[str]
    model: Optional[str]
    temperature: Optional[float]
    max_tokens: Optional[int]
    top_p: Optional[float]
    frequency_penalty: Optional[float]
    presence_penalty: Optional[float]
    def __init__(self) -> None: ...

class LlmStream:
    """Async iterator of text chunks yielded by ``ColmenaLlm.stream``."""

    def __aiter__(self) -> "LlmStream": ...
    async def __anext__(self) -> str: ...

class ColmenaLlm:
    """Multi-provider LLM client. Loads API keys from the environment on init."""

    def __init__(self) -> None: ...
    def call(
        self,
        messages: List[Dict[str, str]],
        provider: str,
        options: Optional[LlmConfigOptions] = ...,
    ) -> str:
        """Run a single LLM call and return the response text.

        ``messages`` is a list of ``{"role", "content"}`` dicts; ``provider`` is
        one of ``"openai"``, ``"google"``, ``"anthropic"``.
        """

    def stream(
        self,
        messages: List[Dict[str, str]],
        provider: str,
        options: Optional[LlmConfigOptions] = ...,
    ) -> Awaitable[LlmStream]:
        """Return an awaitable that resolves to an async iterator of text chunks.

        Usage: ``stream = await llm.stream(...)`` then ``async for chunk in stream``.
        """

    def health_check(self, provider: str) -> bool: ...
    def get_providers(self) -> List[str]: ...

class Registry:
    """Read-only handle to the node registry (no DB connection)."""

    def node_types(self) -> List[str]: ...
    def toolkit_catalog(self, node_type: str, config: Any) -> List[Dict[str, Any]]: ...

def run_dag(
    graph: Union[str, Dict[str, Any]],
    resume_id: Optional[str] = ...,
    resume_answer: Optional[str] = ...,
    inject_payload: Optional[Any] = ...,
    include_extra_info: bool = ...,
    agent_session_id: Optional[str] = ...,
) -> str:
    """Run a DAG graph to completion and return the final output as a JSON string.

    ``graph`` is either a path to a JSON file or an in-memory graph dict.
    """

def validate_graph(graph: Dict[str, Any]) -> None:
    """Validate a graph dict; raise ``DagException`` if it is not a valid graph."""

def serve_dag(file_path: str, host: str = ..., port: int = ...) -> None:
    """Serve a graph's webhook triggers as a (blocking) HTTP API."""

def default_registry() -> Registry:
    """Build an inspection-only node registry with no database connection."""
