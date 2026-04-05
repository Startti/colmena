# --- Multi-stage build for Colmena DAG Engine ---

# 🏗️ Build Stage
FROM rust:1.75-slim-bookworm as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/colmena

# Copy the entire project
COPY . .

# Build the dag_engine binary in release mode
# Note: Using --manifest-path because Cargo.toml is in the root as a workspace
# but the binary is inside src/libs/colmena
RUN cargo build --release --bin dag_engine --manifest-path src/libs/colmena/Cargo.toml

# 🏁 Runtime Stage
FROM debian:bookworm-slim as runtime

# Install runtime dependencies (OpenSSL, CA certificates)
RUN apt-get update && apt-get install -y \
    openssl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the binary from the builder stage
COPY --from=builder /usr/src/colmena/target/release/dag_engine /app/dag_engine

# Ensure the binary is executable
RUN chmod +x /app/dag_engine

# Expose the default port for the engine
EXPOSE 3000

# Run the engine
# Usage: ./dag_engine serve <graph.json> --host 0.0.0.0 --port 3000
ENTRYPOINT ["/app/dag_engine"]
CMD ["serve", "/app/graph.json", "--host", "0.0.0.0", "--port", "3000"]
