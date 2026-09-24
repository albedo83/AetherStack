export type FrameRole = "bias" | "dark" | "flat" | "light";

export type ReviewState = "undecided" | "accepted" | "rejected";

export type MetricValue = number | null;

export type BayerPattern = "rggb" | "bggr" | "grbg" | "gbrg";

export type QualityState =
  "unavailable" | "idle" | "loading" | "ready" | "error";

export interface FrameMetrics {
  readonly fwhmPixels: MetricValue;
  readonly eccentricity: MetricValue;
  readonly detectedStars: number | null;
  readonly background: MetricValue;
  readonly noise: MetricValue;
}

export interface ReviewFrame {
  readonly id: string;
  readonly label: string;
  /** Absolute runtime-only source path; never persisted in portable state. */
  readonly sourcePath: string | null;
  readonly exposureSeconds: number | null;
  readonly temperatureCelsius: number | null;
  readonly classificationWarning: string | null;
  readonly bayerPattern: BayerPattern | null;
  readonly qualityState: QualityState;
  readonly qualityMessage: string;
  readonly qualityProfileId: string | null;
  readonly state: ReviewState;
  readonly rejectionReason: string | null;
  readonly metrics: FrameMetrics;
}

export interface SessionStatus {
  readonly tone: "ready" | "busy" | "warning" | "error";
  readonly label: string;
}

export interface RoleSummary {
  readonly role: FrameRole;
  readonly label: string;
  readonly count: number;
  readonly unresolved: number;
}

export interface FramePreview {
  /** Stable frame identity that the decoded pixels were requested for. */
  readonly frameId: string;
  /** Browser object URL for a display-only artifact produced by Rust. */
  readonly url: string;
}

/** Exact three-pass moments for the complete primary FITS array. */
export interface FitsStatistics {
  readonly algorithmId: string;
  readonly axes: readonly number[];
  readonly storedFormat: string;
  readonly headerConformant: boolean;
  readonly headerDiagnostics: number;
  readonly totalSamples: number;
  readonly usableSamples: number;
  readonly undefinedSamples: number;
  readonly nonFiniteSamples: number;
  readonly minimum: number;
  readonly maximum: number;
  readonly mean: number;
  readonly populationStandardDeviation: number;
  readonly sampleStandardDeviation: number | null;
}

export interface StatisticsPanel {
  readonly open: boolean;
  readonly frameId: string | null;
  readonly frameLabel: string | null;
  readonly state: "idle" | "loading" | "ready" | "error";
  readonly statistics: FitsStatistics | null;
  readonly message: string | null;
}

export interface ReviewViewModel {
  readonly sessionName: string;
  readonly sessionStatus: SessionStatus;
  readonly roles: readonly RoleSummary[];
  readonly activeRole: FrameRole;
  readonly frames: readonly ReviewFrame[];
  readonly selectedFrameId: string | null;
  readonly playing: boolean;
  readonly sharedStretchLabel: string;
  readonly preview: FramePreview | null;
  readonly viewerScale: "fit" | "actual";
  readonly statisticsPanel: StatisticsPanel;
}

export type SortField =
  | "processing_order"
  | "label"
  | "fwhm_major"
  | "eccentricity"
  | "detected_stars"
  | "background";

export type SortDirection = "ascending" | "descending";

export interface ReviewActions {
  readonly onImportSession: () => void;
  readonly onSelectRole: (role: FrameRole) => void;
  readonly onSelectFrame: (frameId: string) => void;
  readonly onSort: (field: SortField, direction: SortDirection) => void;
  readonly onSetDecision: (
    frameId: string,
    state: Exclude<ReviewState, "undecided">,
    reason: string | null,
  ) => void;
  readonly onClearDecision: (frameId: string) => void;
  readonly onUndo: () => void;
  readonly onSetPlaying: (playing: boolean) => void;
  readonly onRequestStep: (direction: "backward" | "forward") => void;
  readonly onSetViewerScale: (scale: "fit" | "actual") => void;
  readonly onOpenStatistics: (frameId: string) => void;
  readonly onCloseStatistics: () => void;
  readonly onMeasureQuality: (frameId: string) => void;
}
