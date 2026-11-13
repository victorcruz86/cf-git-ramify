// Ramify's own (`Start the commit message with the ticket from the branch name`, Settings > Ramify;
// `docs/parity-matrix.md`): an empty commit message starts with the ticket of the branch name, the
// first upper-case key, a dash and a number (`bugfix/HT-730-partner…` → `HT-730`), followed by a
// space, as the team's commits read (`HT-830 Use TLS…`).
const TICKET = /(?:^|[^A-Za-z0-9])([A-Z][A-Z0-9]*-[0-9]+)(?![A-Za-z0-9])/;

export function ticketOf(branch: string | null | undefined): string | null {
  return branch ? (TICKET.exec(branch)?.[1] ?? null) : null;
}

export const ticketPrefix = (branch: string | null | undefined): string => {
  const ticket = ticketOf(branch);
  return ticket ? `${ticket} ` : "";
};

// Empty, or nothing but the ticket: it counts as no message (`Specify commit message`), and it is
// not kept as a changelist's comment.
export function isBare(message: string, ticket: string | null): boolean {
  const text = message.trim();
  return text === "" || (ticket !== null && text === ticket);
}
