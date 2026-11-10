import { logSignatures, taskCancel, type Signature, type SignatureStatus } from "../lib/api";
import { taskResult } from "../lib/tasks";

// GitBundle (`GitB:1084-1100`).
export const SIGNATURE_COLUMN = "GPG Signature";
export const NO_SIGNATURE = "No GPG signature";
const BAD = "Bad GPG signature";
const UNVERIFIED = "Unverified GPG signature";
const VERIFIED = "Verified GPG signature";
export const FINGERPRINT = "Signature fingerprint:";
export const SIGNED_BY = "Signed by:";
const REASONS: Record<Exclude<SignatureStatus, "verified" | "bad">, string> = {
  unknown: "unknown validity",
  expired: "expired signature",
  expiredKey: "made with expired key",
  revokedKey: "made with revoked key",
  cannotVerify: "cannot verify (e.g. missing key)",
};

// `VcsLogExternalStatusColumnService.loadDataForVisibleRows`: the visible rows and 15 more on each
// side, 300 ms after the last change.
export const SIGNATURE_ROWS_AROUND = 15;
export const SIGNATURE_DELAY_MS = 300;
// `CachingVcsCommitsDataLoader`: what one table keeps.
const TABLE_CACHE = 150;

export type SignatureIcon = "signatureVerified" | "signatureSigned";

// `GitCommitSignatureStatusPresentation.icon`. Unsigned, or not loaded yet, is the empty icon.
export function signatureIcon(signature: Signature | undefined): SignatureIcon | null {
  if (!signature) return null;
  return signature.status === "verified" ? "signatureVerified" : "signatureSigned";
}

// `.text`: the cell's tooltip and the text in the details. An unsigned commit, and one not loaded
// yet, takes the stub status (`getStubStatus`).
export function signatureText(signature: Signature | undefined): string {
  if (!signature) return NO_SIGNATURE;
  if (signature.status === "verified") return VERIFIED;
  return signature.status === "bad" ? BAD : UNVERIFIED;
}

// `.description`: the tooltip of the icon in the details. The fingerprint and the signer only for a
// verified signature, in the context-help colour.
export interface SignatureDescription {
  text: string;
  verified: { fingerprint: string; signer: string } | null;
}

export function signatureDescription(signature: Signature): SignatureDescription {
  const { status, fingerprint, signer } = signature;
  if (status === "verified") return { text: VERIFIED, verified: { fingerprint, signer } };
  if (status === "bad") return { text: BAD, verified: null };
  return { text: `${UNVERIFIED}: ${REASONS[status]}`, verified: null };
}

let lastRequester = 0;

// `GitCommitSignatureLoaderBase`: one loader per table or details panel. Its new request cancels
// its previous one (the backend does it per `requester`), and what a cancelled or failed request
// answers is dropped: 241 only logs it.
export class SignatureLoader {
  readonly requester = ++lastRequester;
  private running: number | null = null;
  private token = 0;

  async load(hashes: string[]): Promise<Signature[] | null> {
    if (hashes.length === 0) {
      this.cancel();
      return [];
    }
    const token = ++this.token;
    let id: number;
    try {
      id = await logSignatures(hashes, this.requester);
    } catch {
      return null;
    }
    if (token !== this.token) {
      // `cancel` came while the task was being started: no later request will cancel it.
      taskCancel(id).catch(() => {});
      return null;
    }
    this.running = id;
    try {
      const found = await taskResult<Signature[]>(id);
      return token === this.token ? found : null;
    } catch {
      return null;
    } finally {
      if (this.running === id) this.running = null;
    }
  }

  cancel() {
    this.token++;
    if (this.running !== null) taskCancel(this.running).catch(() => {});
    this.running = null;
  }
}

// What the `GPG Signature` column of one table knows, and when it asks for more: only while the
// column is shown, for the rows `rows` names, after `SIGNATURE_DELAY_MS` without another change.
// Asking never waits for anything: the table paints with what is known and repaints on `onchange`.
export class SignatureColumn {
  private readonly loader = new SignatureLoader();
  private readonly known = new Map<string, Signature>();
  private wanted: string | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly onchange: () => void) {}

  get(hash: string): Signature | undefined {
    return this.known.get(hash);
  }

  // `rows` says which rows they are (and whether they are loaded): the same as last time asks
  // nothing. `hashes` is read when the delay ends.
  request(shown: boolean, rows: string, hashes: () => string[]) {
    if (!shown) {
      this.stop();
      return;
    }
    if (rows === this.wanted) return;
    this.wanted = rows;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.load(hashes());
    }, SIGNATURE_DELAY_MS);
  }

  // A hidden column asks for nothing, and what it was asking for is cancelled.
  stop() {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.wanted = null;
    this.loader.cancel();
  }

  private async load(hashes: string[]) {
    const found = await this.loader.load(hashes);
    if (!found || found.length === 0) return;
    for (const signature of found) {
      this.known.delete(signature.hash);
      this.known.set(signature.hash, signature);
    }
    while (this.known.size > TABLE_CACHE) this.known.delete(this.known.keys().next().value!);
    this.onchange();
  }
}
