/**
 * Shares one asynchronous attempt among concurrent callers, then clears it on
 * either success or failure so a later caller can become the next owner.
 */
export class SharedAttemptGate<T> {
  private active: Promise<T> | null = null;

  current(): Promise<T> | null {
    return this.active;
  }

  run(factory: () => Promise<T>): Promise<T> {
    if (this.active) return this.active;

    const attempt = factory();
    const tracked = attempt.finally(() => {
      if (this.active === tracked) this.active = null;
    });
    this.active = tracked;
    return tracked;
  }
}
