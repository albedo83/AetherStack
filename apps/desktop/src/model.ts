export type FrameRole = "bias" | "dark" | "flat" | "light";

export type ReviewState = "undecided" | "accepted" | "rejected";

export type MetricValue = number | null;

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
  readonly exposureSeconds: number;
  readonly temperatureCelsius: number | null;
  readonly state: ReviewState;
  readonly rejectionReason: string | null;
  readonly metrics: FrameMetrics;
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

export interface ReviewViewModel {
  readonly sessionName: string;
  readonly roles: readonly RoleSummary[];
  readonly activeRole: FrameRole;
  readonly frames: readonly ReviewFrame[];
  readonly selectedFrameId: string | null;
  readonly playing: boolean;
  readonly sharedStretchLabel: string;
  readonly preview: FramePreview | null;
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
}
