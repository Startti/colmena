#!/usr/bin/env python3
"""
Test for mock streaming to ensure the Rust->Python streaming bridge works as expected.
"""
try:
    import colmena
    print("✓ colmena module imported successfully")
except ImportError as e:
    print(f"✗ Error importing colmena: {e}")
    exit(1)

def test_mock_streaming_sequentially():
    """
    Tests that the mock streaming function yields items sequentially
    and that prints from Rust and Python are interleaved as expected.
    """
    print("\n📋 Running test_mock_streaming_sequentially")
    llm = colmena.ColmenaLlm()
    
    print("🐍 [Python] Calling mock_stream...")
    stream = llm.mock_stream()
    
    print("🐍 [Python] Iterating over stream...")
    
    expected_chunks = ["this", "is", "an", "stremaing", "mock"]
    received_chunks = []

    for chunk in stream:
        print(f"🐍 [Python] Received chunk: '{chunk}'")
        received_chunks.append(chunk)
        
    print("\n🐍 [Python] Finished iterating.")
    
    print(f"\nReceived chunks: {received_chunks}")
    print(f"Expected chunks: {expected_chunks}")

    if received_chunks == expected_chunks:
        print("✅ PASSED: Received chunks match expected chunks.")
        return True
    else:
        print("❌ FAILED: Received chunks do not match expected chunks.")
        return False

if __name__ == "__main__":
    print("🧪 Mock Streaming Testing")
    print("="*60)
    
    passed = test_mock_streaming_sequentially()
    
    print("="*60)
    if not passed:
        exit(1)
