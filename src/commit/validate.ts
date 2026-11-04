export type MessageIssueCode =
  | "empty"
  | "subject-too-long"
  | "second-line-not-empty"
  | "body-too-long";

// `start` and `end` are columns of `line`: the stretch that gets underlined.
export interface MessageIssue {
  code: MessageIssueCode;
  line: number;
  start: number;
  end: number;
  text: string;
}

const RIGHT_MARGIN = 72;

export function validateMessage(message: string): MessageIssue[] {
  if (!message.trim()) {
    return [{ code: "empty", line: 0, start: 0, end: 0, text: "Specify commit message" }];
  }
  const lines = message.split("\n");
  const issues: MessageIssue[] = [];
  lines.forEach((line, index) => {
    if (line.length > RIGHT_MARGIN) {
      issues.push(tooLong(line, index));
    }
  });
  if (lines.length > 1 && lines[1].length > 0) {
    issues.push({
      code: "second-line-not-empty",
      line: 1,
      start: 0,
      end: lines[1].length,
      text: "Missing blank line between subject and body",
    });
  }
  return issues;
}

function tooLong(line: string, index: number): MessageIssue {
  const subject = index === 0;
  return {
    code: subject ? "subject-too-long" : "body-too-long",
    line: index,
    start: RIGHT_MARGIN,
    end: line.length,
    text: `${subject ? "Subject" : "Body lines"} cannot exceed ${RIGHT_MARGIN} characters`,
  };
}

export interface MessageSegment {
  text: string;
  marked: boolean;
}

// The message split into segments, to draw the inspections' underline below the field.
export function highlightMessage(message: string, issues: MessageIssue[]): MessageSegment[] {
  const segments: MessageSegment[] = [];
  const push = (text: string, marked: boolean) => {
    if (!text) return;
    const last = segments[segments.length - 1];
    if (last && last.marked === marked) last.text += text;
    else segments.push({ text, marked });
  };
  message.split("\n").forEach((line, index, lines) => {
    const issue = issues.find((item) => item.line === index);
    push(issue ? line.slice(0, issue.start) : line, false);
    if (issue) push(line.slice(issue.start, issue.end), true);
    if (index < lines.length - 1) push("\n", false);
  });
  return segments;
}
