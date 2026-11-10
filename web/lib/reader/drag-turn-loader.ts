/**
 * On-demand loader for the drag-to-turn motion (`drag-turn-engine.ts`).
 * Its own module so the reader's first-load chunk holds only this
 * `import()` (and tests can hold the load pending or fail it).
 */
export const loadDragTurnEngine = () => import("./drag-turn-engine");
