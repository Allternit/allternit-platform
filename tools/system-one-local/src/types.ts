// Wire types for the System One contract (docs.typesafe.ai/api), plus the
// local `x_allternit` response extension. Clients that only know the official
// contract can ignore the extension field.

export type Structured = string | Record<string, unknown> | unknown[];

export interface NoulQuestion {
  type: "noul";
  instructions: Structured;
  criteria?: { true?: Structured; false?: Structured };
}

export interface ChoiceQuestion {
  type: "choice";
  instructions: Structured;
  criteria: Record<string, Structured | null>;
}

export interface ScoreQuestion {
  type: "score";
  instructions: Structured;
  criteria: Structured[];
}

export type Question = NoulQuestion | ChoiceQuestion | ScoreQuestion;

export interface SystemOneRequest {
  model: string;
  state: Structured;
  questions: Record<string, Question>;
}

export interface NoulAnswer {
  type: "noul";
  noul: number;
}

export interface ChoiceAnswer {
  type: "choice";
  choice: string;
  probabilities: Record<string, number>;
  confidence: number;
}

export interface ScoreAnswer {
  type: "score";
  score: number;
  legend: Record<string, string>;
  probabilities: Record<string, number>;
  confidence: number;
}

export type Answer = NoulAnswer | ChoiceAnswer | ScoreAnswer;

/** How a probability distribution was obtained for one question. */
export type Method = "logprobs" | "sampled" | "remote";

export interface Extension {
  backend: string;
  runtime?: string;
  methods: Record<string, Method>;
  /** Share of top-k probability mass that landed on valid labels, per question (logprobs only). */
  label_mass?: Record<string, number>;
  latency_ms: number;
}

export interface SystemOneResponse {
  model: string;
  answers: Record<string, Answer>;
  usage: { input_tokens: number; output_tokens: number };
  x_allternit?: Extension;
}

export interface ErrorBody {
  error: {
    type:
      | "authentication_error"
      | "invalid_request_error"
      | "rate_limit_error"
      | "overloaded_error"
      | "not_found_error"
      | "api_error";
    message: string;
    details?: { path: string; message: string }[];
  };
}

export class SystemOneError extends Error {
  constructor(
    public status: 401 | 404 | 422 | 429 | 500 | 529,
    public body: ErrorBody,
  ) {
    super(body.error.message);
  }
}
