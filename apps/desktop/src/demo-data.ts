import type { ReviewViewModel } from "./model.ts";

const frameId = (digit: string): string => digit.repeat(64);

/**
 * Synthetic development data for rendering the shell before the Tauri command
 * bridge is connected. It contains no acquisition path or private metadata.
 */
export const demoReviewModel: ReviewViewModel = {
  sessionName: "M31 · Session 01",
  sessionStatus: { tone: "ready", label: "Demo ready" },
  activeRole: "light",
  playing: false,
  selectedFrameId: frameId("b"),
  sharedStretchLabel: "Shared stretch · locked",
  preview: null,
  viewerScale: "fit",
  statisticsPanel: {
    open: false,
    frameId: null,
    frameLabel: null,
    state: "idle",
    statistics: null,
    message: null,
  },
  roles: [
    { role: "bias", label: "Bias", count: 0, unresolved: 0 },
    { role: "dark", label: "Darks", count: 204, unresolved: 0 },
    { role: "flat", label: "Flats", count: 26, unresolved: 0 },
    { role: "light", label: "Lights", count: 10, unresolved: 0 },
  ],
  frames: [
    {
      id: frameId("a"),
      label: "light_0001.fits",
      sourcePath: null,
      exposureSeconds: 60,
      temperatureCelsius: -5.3,
      classificationWarning: null,
      state: "accepted",
      rejectionReason: null,
      metrics: {
        fwhmPixels: 3.21,
        eccentricity: 0.39,
        detectedStars: 842,
        background: 1912.4,
        noise: 18.7,
      },
    },
    {
      id: frameId("b"),
      label: "light_0002.fits",
      sourcePath: null,
      exposureSeconds: 60,
      temperatureCelsius: -4.9,
      classificationWarning: null,
      state: "undecided",
      rejectionReason: null,
      metrics: {
        fwhmPixels: 3.38,
        eccentricity: 0.42,
        detectedStars: 817,
        background: 1921.8,
        noise: 19.1,
      },
    },
    {
      id: frameId("c"),
      label: "light_0003.fits",
      sourcePath: null,
      exposureSeconds: 60,
      temperatureCelsius: -4.9,
      classificationWarning: null,
      state: "rejected",
      rejectionReason: "Trailing",
      metrics: {
        fwhmPixels: 5.84,
        eccentricity: 0.71,
        detectedStars: 503,
        background: 1938.2,
        noise: 23.6,
      },
    },
    {
      id: frameId("d"),
      label: "light_0004.fits",
      sourcePath: null,
      exposureSeconds: 60,
      temperatureCelsius: -5.3,
      classificationWarning: null,
      state: "undecided",
      rejectionReason: null,
      metrics: {
        fwhmPixels: 3.46,
        eccentricity: 0.44,
        detectedStars: 791,
        background: 1927.1,
        noise: 19.8,
      },
    },
    {
      id: frameId("e"),
      label: "light_0005.fits",
      sourcePath: null,
      exposureSeconds: 60,
      temperatureCelsius: -5.3,
      classificationWarning: null,
      state: "undecided",
      rejectionReason: null,
      metrics: {
        fwhmPixels: null,
        eccentricity: null,
        detectedStars: null,
        background: 1920.9,
        noise: 19.2,
      },
    },
  ],
};
