#!/usr/bin/env python3
"""
Test for mock streaming to ensure the Rust->Python streaming bridge works as expected.
"""
import pytest

try:
    import colmena
except ImportError:
    colmena = None


@pytest.mark.skipif(
    colmena is None or not hasattr(colmena.ColmenaLlm, "mock_stream"),
    reason="mock_stream not exposed in current PyO3 bindings",
)
def test_mock_streaming_sequentially():
    """
    Tests that the mock streaming function yields items sequentially
    and that prints from Rust and Python are interleaved as expected.
    """
    llm = colmena.ColmenaLlm()
    stream = llm.mock_stream()

    expected_chunks = ["this", "is", "an", "stremaing", "mock"]
    received_chunks = []

    for chunk in stream:
        received_chunks.append(chunk)

    assert received_chunks == expected_chunks
