# oxo-flow E2E Tests (Playwright)

End-to-end browser tests for the oxo-flow multi-user web system.

## Setup

```bash
npm install
npx playwright install
```

## Running Tests

```bash
# Run all tests
npm test

# Run with visible browser
npm run test:headed

# Run specific test file
npx playwright test auth.spec.ts

# Run multi-user simulation
npx playwright test multi-user-simulation.spec.ts

# Debug mode
npm run test:debug

# View test report
npm run report
```

## Test Coverage

- **auth.spec.ts**: Login/logout, session validation
- **dashboard.spec.ts**: Metrics display, recent runs
- **workflow-editor.spec.ts**: Validate, dry-run, format, lint, DAG, export, run
- **runs.spec.ts**: Run history, logs, cancel
- **saved-workflows.spec.ts**: Save/load/delete workflows
- **system.spec.ts**: Environment info, version
- **multi-user-simulation.spec.ts**: 20-user concurrent access simulation

## CI Integration

Tests run automatically in CI with:
- Chromium, Firefox, WebKit browsers
- Retry on failure (2 retries)
- HTML report generation