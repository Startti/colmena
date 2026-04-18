#!/usr/bin/env python3
"""
Test for async mock streaming to ensure the Rust->Python async streaming bridge works.
"""
import pytest

try:
    import colmena
except ImportError:
    colmena = None


@pytest.mark.skipif(
    colmena is None or not hasattr(colmena.ColmenaLlm, "mock_stream_async"),
    reason="mock_stream_async not exposed in current PyO3 bindings",
)
async def test_async_mock_streaming_sequentially():
    """
    Tests that the async mock streaming function yields items sequentially
    and that prints from Rust and Python are interleaved as expected.
    """
    llm = colmena.ColmenaLlm()
    stream = await llm.mock_stream_async()

    expected_chunks = ["this", "is", "an", "async", "mock"]
    received_chunks = []

    async for chunk in stream:
        received_chunks.append(chunk)

    assert received_chunks == expected_chunks
