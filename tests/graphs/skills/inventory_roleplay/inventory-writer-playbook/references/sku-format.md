# SKU format

- Pattern: `[A-Z]{3}-[0-9]{3,4}` (3 uppercase letters, dash, 3-4 digits).
- Examples: `ABC-001`, `XYZ-9999`.
- Invalid: `abc-1`, `AB-12`, `ABCD-001`, `ABC_001`.
- When the user supplies a SKU that doesn't match, refuse the operation and ask them to correct it.
