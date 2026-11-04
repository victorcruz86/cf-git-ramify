export interface Range {
  start: number;
  end: number;
}

export function visibleRange(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  total: number,
  overscan: number,
): Range {
  const first = Math.floor(scrollTop / rowHeight);
  const last = Math.ceil((scrollTop + viewportHeight) / rowHeight);
  return {
    start: Math.min(total, Math.max(0, first - overscan)),
    end: Math.min(total, last + overscan),
  };
}

export class RowCache<T> {
  private pages = new Map<number, T[]>();
  private inFlight = new Map<number, Promise<void>>();

  constructor(
    private fetchPage: (start: number, count: number) => Promise<T[]>,
    private pageSize = 200,
    private maxPages = 20,
  ) {}

  get pageCount(): number {
    return this.pages.size;
  }

  async ensure(start: number, end: number): Promise<void> {
    const firstPage = Math.floor(start / this.pageSize);
    const lastPage = Math.floor((end - 1) / this.pageSize);
    const loads: Promise<void>[] = [];
    for (let page = firstPage; page <= lastPage; page++) {
      loads.push(this.load(page));
    }
    await Promise.all(loads);
  }

  loaded(index: number): boolean {
    return this.pages.has(Math.floor(index / this.pageSize));
  }

  get(index: number): T | undefined {
    const page = this.pages.get(Math.floor(index / this.pageSize));
    return page?.[index % this.pageSize];
  }

  private load(page: number): Promise<void> {
    const existing = this.pages.get(page);
    if (existing) {
      // Reinserting marks the page as recently used.
      this.pages.delete(page);
      this.pages.set(page, existing);
      return Promise.resolve();
    }
    const pending = this.inFlight.get(page);
    if (pending) return pending;

    const request = this.fetchPage(page * this.pageSize, this.pageSize)
      .then((rows) => {
        this.pages.set(page, rows);
        this.evict();
      })
      .finally(() => this.inFlight.delete(page));
    this.inFlight.set(page, request);
    return request;
  }

  private evict(): void {
    while (this.pages.size > this.maxPages) {
      const oldest = this.pages.keys().next().value as number;
      this.pages.delete(oldest);
    }
  }
}
