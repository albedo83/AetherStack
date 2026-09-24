import "./styles.css";

import { demoReviewModel } from "./demo-data.ts";
import type { ReviewState, ReviewViewModel } from "./model.ts";
import { mountReviewScreen } from "./review-screen.ts";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("AetherStack desktop root is missing");

let model = demoReviewModel;

const screen = mountReviewScreen(root, model, {
  onSelectRole(role) {
    if (role === "light") {
      update({
        ...model,
        activeRole: role,
        frames: demoReviewModel.frames,
        selectedFrameId: demoReviewModel.selectedFrameId,
      });
      return;
    }
    update({ ...model, activeRole: role, frames: [], selectedFrameId: null });
  },
  onSelectFrame(frameId) {
    update({ ...model, selectedFrameId: frameId });
  },
  onSort() {
    // Sorting is deliberately deferred to the Rust review model. The demo data
    // remains in processing order so the frontend cannot imply a reorder.
  },
  onSetDecision(frameId, state, reason) {
    updateDecision(frameId, state, reason);
  },
  onClearDecision(frameId) {
    updateDecision(frameId, "undecided", null);
  },
  onUndo() {
    // The production adapter will request transactional undo from Rust.
  },
  onSetPlaying(playing) {
    update({ ...model, playing });
  },
  onRequestStep(direction) {
    const current = model.frames.findIndex(
      (frame) => frame.id === model.selectedFrameId,
    );
    if (current < 0 || model.frames.length < 2) return;
    const offset = direction === "forward" ? 1 : -1;
    const next = (current + offset + model.frames.length) % model.frames.length;
    const frame = model.frames[next];
    if (frame) update({ ...model, selectedFrameId: frame.id });
  },
});

function update(next: ReviewViewModel): void {
  model = next;
  screen.update(model);
}

function updateDecision(
  frameId: string,
  state: ReviewState,
  reason: string | null,
): void {
  update({
    ...model,
    frames: model.frames.map((frame) =>
      frame.id === frameId
        ? { ...frame, state, rejectionReason: reason }
        : frame,
    ),
  });
}
