# 📦 Deployment y Distribución

### Building Wheels

```bash
# Build para múltiples plataformas
maturin build --release --target x86_64-unknown-linux-gnu
maturin build --release --target x86_64-pc-windows-msvc
maturin build --release --target x86_64-apple-darwin
maturin build --release --target aarch64-apple-darwin

# Wheels se crean en target/wheels/
ls target/wheels/
```

### GitHub Actions CI/CD

```yaml
# .github/workflows/ci.yml
name: CI

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        python-version: [3.8, 3.9, "3.10", "3.11"]

    steps:
    - uses: actions/checkout@v3

    - name: Set up Python ${{ matrix.python-version }}
      uses: actions/setup-python@v4
      with:
        python-version: ${{ matrix.python-version }}

    - name: Set up Rust
      uses: actions-rs/toolchain@v1
      with:
        toolchain: stable
        override: true

    - name: Install dependencies
      run: |
        python -m pip install --upgrade pip
        pip install maturin pytest

    - name: Build
      run: maturin develop --release

    - name: Test
      run: pytest tests/

  build:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest, macos-latest]

    steps:
    - uses: actions/checkout@v3

    - name: Set up Python
      uses: actions/setup-python@v4
      with:
        python-version: "3.11"

    - name: Set up Rust
      uses: actions-rs/toolchain@v1
      with:
        toolchain: stable
        override: true

    - name: Build wheels
      run: |
        pip install maturin
        maturin build --release

    - name: Upload wheels
      uses: actions/upload-artifact@v3
      with:
        name: wheels
        path: target/wheels/
```

### Release Process

```bash
# scripts/release.sh
#!/bin/bash

set -e

VERSION=$1
if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>"
    exit 1
fi

echo "🚀 Releasing version $VERSION"

# 1. Update version in Cargo.toml
sed -i "s/^version = .*/version = \"$VERSION\"/" Cargo.toml

# 2. Update version in pyproject.toml
sed -i "s/^version = .*/version = \"$VERSION\"/" pyproject.toml

# 3. Run tests
cargo test
python -m pytest

# 4. Build wheels for all platforms
maturin build --release

# 5. Create git tag
git add .
git commit -m "Release v$VERSION"
git tag "v$VERSION"

# 6. Push to repository
git push origin main
git push origin "v$VERSION"

# 7. Upload to PyPI (manual step)
echo "✅ Release prepared. Run 'maturin publish' to upload to PyPI"
```

### Versioning Strategy

```toml
# Cargo.toml
[package]
version = "0.1.0"    # SemVer: MAJOR.MINOR.PATCH

# 0.x.y = Pre-1.0, breaking changes allowed
# 1.x.y = Stable API, breaking changes require major version bump
# x.y.Z = Bug fixes only
# x.Y.z = New features, backward compatible
# X.y.z = Breaking changes
```

