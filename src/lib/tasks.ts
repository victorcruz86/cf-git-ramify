import { writable } from "svelte/store";
import { onTaskChanged, type ApiError, type TaskEvent } from "./api";

// The tasks in progress. One that finishes, in whatever way, stops being there; its result is
// picked up by `taskResult`.
const store = writable<TaskEvent[]>([]);

export const tasks = { subscribe: store.subscribe };

// The command that launches a task returns its id when the task may already have finished: the
// last events of the finished ones are kept so that `taskResult` finds them even if it arrives late.
const KEPT = 200;
const finished = new Map<number, TaskEvent>();
const waiting = new Map<number, ((event: TaskEvent) => void)[]>();

function apply(event: TaskEvent) {
  store.update((running) => {
    const others = running.filter((task) => task.id !== event.id);
    if (event.state !== "running") return others;
    const index = running.findIndex((task) => task.id === event.id);
    return index < 0 ? [...others, event] : running.map((task, i) => (i === index ? event : task));
  });
  if (event.state === "running") return;
  finished.set(event.id, event);
  if (finished.size > KEPT) finished.delete(finished.keys().next().value!);
  waiting.get(event.id)?.forEach((settle) => settle(event));
  waiting.delete(event.id);
}

// Work of this window that is not a backend task, such as the commit that runs once the Commit
// Changes dialog has closed. It is shown while it runs, with negative ids and without Cancel.
let lastLocal = 0;

export const isLocal = (task: TaskEvent) => task.id < 0;

export async function runLocal<T>(title: string, text: string, work: () => Promise<T>): Promise<T> {
  const id = --lastLocal;
  const shown: TaskEvent = {
    id,
    title,
    text,
    fraction: null,
    state: "running",
    kind: null,
    value: null,
  };
  store.update((running) => [...running, shown]);
  try {
    return await work();
  } finally {
    store.update((running) => running.filter((task) => task.id !== id));
  }
}

export async function trackTasks(): Promise<() => void> {
  store.set([]);
  return onTaskChanged(apply);
}

// What the task returned. If it fails or is cancelled it rejects with the shape of `ApiError`: the
// `kind` says why (for example "cancelled").
export function taskResult<T>(id: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const settle = (event: TaskEvent) => {
      if (event.state === "done") resolve(event.value as T);
      else {
        const message = event.text || "The operation was cancelled";
        reject({ kind: event.kind ?? "failed", message } satisfies ApiError);
      }
    };
    const known = finished.get(id);
    if (known) settle(known);
    else waiting.set(id, [...(waiting.get(id) ?? []), settle]);
  });
}
