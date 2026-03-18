---
name: python_dev
description: Protocol for Python development in this repository. Use when modifying or creating Python code and scripts.
---

# Python Development Skill

Detailed instructions for Python development, ensuring consistency in environment management, planning, and code review.

## When to use this skill

- Use this when modifying any Python (`.py`) files.
- Use this when creating new Python scripts or modules.
- Use this when managing dependencies or virtual environments.

## How to use it

### 1. Environment Management
- **Always** use the virtual environment located at `.venv` in the root of the repository.
- Ensure the venv is activated before running any scripts or installing packages.

### 2. Planning and Validation
- **Requirement**: Always create an `implementation_plan.md` before executing any code changes.
- **Process**:
    - Describe **what** is being changed and **how** it will be implemented.
    - Show the **exact code blocks** and parts of the file that will be modified to facilitate a better review.
    - Submit the plan to the user and wait for explicit approval before proceeding to the execution phase.

### 3. Review Checklist
Every code change must include a review session that addresses the following:
- **Potential Issues**: Identify and list any possible side effects, performance impacts, or bugs introduced by the code.
- **Affected Scripts**: List ALL scripts and modules that are affected by the changes, including dependencies and downstream tasks.
- **Explanations**: Provide a clear explanation of the "what" and the "how" for every significant change.

### 4. Code Standards
- Follow PEP 8 guidelines.
- Ensure proper logging and error handling are implemented.
- Maintain consistency with existing architecture patterns in the repository.
