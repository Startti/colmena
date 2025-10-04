#!/bin/bash
# Install git hooks for conventional commits validation

echo "Installing git hooks..."

# Create hooks directory if it doesn't exist
mkdir -p .git/hooks

# Copy commit-msg hook
cp .husky/commit-msg .git/hooks/commit-msg
chmod +x .git/hooks/commit-msg

echo "✅ Git hooks installed successfully!"
echo ""
echo "Your commits will now be validated against Conventional Commits format:"
echo "  <type>[optional scope][!]: <description>"
echo ""
echo "Examples:"
echo "  feat: add new feature"
echo "  fix(api): resolve bug"
echo "  feat!: breaking change"
