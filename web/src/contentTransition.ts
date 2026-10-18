import { flushSync } from "react-dom";

/** Commit data together without moving or fading the surrounding content. */
export function transitionContent(update: () => void) {
  flushSync(update);
}
