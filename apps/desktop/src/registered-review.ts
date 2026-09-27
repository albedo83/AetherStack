import type { ExecutedCalibratedLightFrame } from "./calibration-bridge.ts";
import type { ExecutedRegisteredFrame } from "./registration-bridge.ts";
import type {
  RegisteredReviewFrame,
  RegistrationFrameOption,
} from "./model.ts";

/**
 * Binds published registration products back to their reviewed identities.
 *
 * The function deliberately rejects partial, duplicate, or foreign sets. The
 * result viewer must never make an incomplete transaction look successful.
 */
export function bindRegisteredReviewFrames(
  reviewed: readonly RegistrationFrameOption[],
  calibrated: readonly ExecutedCalibratedLightFrame[],
  registered: readonly ExecutedRegisteredFrame[],
): readonly RegisteredReviewFrame[] | null {
  if (registered.length === 0 || registered.length !== reviewed.length) {
    return null;
  }
  const calibratedById = uniqueById(calibrated, (frame) => frame.sourceFrameId);
  const registeredById = uniqueById(registered, (frame) => frame.frameId);
  if (!calibratedById || !registeredById) return null;

  const bound: RegisteredReviewFrame[] = [];
  for (const frame of reviewed) {
    const source = calibratedById.get(frame.id);
    const output = registeredById.get(frame.id);
    if (!source || !output || output.outputPath.length === 0) return null;
    bound.push({
      id: frame.id,
      label: frame.label,
      outputPath: output.outputPath,
      previewContent: source.rgbOutputPath
        ? { kind: "rgb" }
        : { kind: "scalar", plane: 0 },
    });
  }
  return bound;
}

function uniqueById<T>(
  values: readonly T[],
  identity: (value: T) => string,
): ReadonlyMap<string, T> | null {
  const indexed = new Map<string, T>();
  for (const value of values) {
    const id = identity(value);
    if (id.length === 0 || indexed.has(id)) return null;
    indexed.set(id, value);
  }
  return indexed;
}
