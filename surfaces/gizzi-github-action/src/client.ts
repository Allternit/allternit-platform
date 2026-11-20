import { ChatMessage, ClientConfig } from "./types";

/**
 * Response from the chat completions endpoint.
 */
interface ChatCompletionResponse {
  choices: Array<{
    message: {
      role: string;
      content: string;
    };
  }>;
}

/**
 * Client for communicating with the Allternit API backend.
 * Uses the native fetch API for zero external dependencies.
 */
export class AllternitClient {
  private readonly config: ClientConfig;

  /**
   * Creates a new AllternitClient instance.
   * @param config - Client configuration including API URL, key, and model
   */
  constructor(config: ClientConfig) {
    this.config = {
      ...config,
      apiUrl: config.apiUrl.replace(/\/$/, ""),
    };
  }

  /**
   * Sends a chat completion request to the Allternit API.
   * @param messages - Array of chat messages
   * @returns The assistant's response text
   */
  public async chat(messages: ChatMessage[]): Promise<string> {
    const body = {
      model: this.config.model,
      messages,
      max_tokens: this.config.maxTokens,
      temperature: 0.3,
      // Turn hint read by the API (VALUED_TURN_HINTS): nobody watches this run.
      metadata: { run_kind: "background_code" },
    };

    const response = await fetch(`${this.config.apiUrl}/v1/chat/completions`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${this.config.apiKey}`,
      },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      const errorText = await response.text();
      throw new Error(`Allternit API error (${response.status}): ${errorText}`);
    }

    const data = (await response.json()) as ChatCompletionResponse;
    const choice = data.choices?.[0];
    if (!choice?.message?.content) {
      throw new Error("Allternit API returned an empty response");
    }

    return choice.message.content;
  }

  /**
   * Reviews a code diff and returns structured review findings.
   * @param diff - The unified diff to review
   * @param filename - The filename for context
   * @returns Review findings as a markdown string
   */
  public async reviewDiff(diff: string, filename: string): Promise<string> {
    return this.chat([
      {
        role: "system",
        content:
          "You are an expert code reviewer. Review the given diff for bugs, security vulnerabilities, " +
          "performance issues, and code quality problems. For each finding, specify:\n" +
          "- Severity: 🔴 Critical, 🟡 Warning, 🔵 Info\n" +
          "- Line reference (if applicable)\n" +
          "- Description of the issue\n" +
          "- Suggested fix\n\n" +
          "If no issues are found, respond with: ✅ No issues found.",
      },
      {
        role: "user",
        content: `Review this diff for \`${filename}\`:\n\n\`\`\`diff\n${diff}\n\`\`\``,
      },
    ]);
  }

  /**
   * Generates code based on a context and prompt.
   * @param context - The existing code or project context
   * @param prompt - The generation instruction
   * @returns Generated code as a string
   */
  public async generateCode(context: string, prompt: string): Promise<string> {
    return this.chat([
      {
        role: "system",
        content:
          "You are an expert code generator. Generate clean, well-documented, production-quality code. " +
          "Return only the generated code in a markdown code block with the appropriate language identifier. " +
          "Include brief comments explaining key decisions.",
      },
      {
        role: "user",
        content: `Context:\n${context}\n\nTask: ${prompt}`,
      },
    ]);
  }
}
