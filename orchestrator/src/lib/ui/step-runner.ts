// Step runner UI (vm0 pattern, ~120 lines)
// Skeleton — will be implemented in N1

/** A step in the step runner */
export interface Step {
  label: string;
  run: () => Promise<void>;
}

/** Run steps sequentially with status display */
export async function runSteps(steps: Step[]): Promise<void> {
  for (const step of steps) {
    console.log(`  → ${step.label}`);
    await step.run();
    console.log(`  ✓ ${step.label}`);
  }
}
