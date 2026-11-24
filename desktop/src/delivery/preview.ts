// Issue reads carry a generation so a superseded URL can never regain a startable preview.
import type { IssuePreview } from "./model";
/** Invalidated responses and errors belong to the discarded URL and are omitted entirely. */
export class IssuePreviewReader {
  private generation = 0;
  constructor(private readonly fetch: (url: string) => Promise<IssuePreview>) {}
  /** Edits and unmounts revoke all earlier previews without cancelling another action's receipt. */
  invalidate() { this.generation++; }
  /** Only the latest URL/request can update the form, including its error and pending state. */
  async read(url: string): Promise<{preview?:IssuePreview;error?:string}|undefined> {
    const generation = ++this.generation;
    try { const preview = await this.fetch(url); return generation === this.generation ? { preview } : undefined; }
    catch(error) { return generation === this.generation ? { error: String(error) } : undefined; }
  }
}
