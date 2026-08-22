// Run with: node --test src/utils/sharedAttemptGate.test.ts

import test from 'node:test';
import assert from 'node:assert/strict';

import { SharedAttemptGate } from './sharedAttemptGate.ts';

test('concurrent callers wait for the same attempt', async () => {
  const gate = new SharedAttemptGate<number>();
  let resolve!: (value: number) => void;
  let calls = 0;
  const first = gate.run(
    () =>
      new Promise<number>((done) => {
        calls++;
        resolve = done;
      }),
  );
  const second = gate.run(async () => {
    calls++;
    return 99;
  });

  assert.equal(first, second);
  assert.equal(calls, 1);
  resolve(42);
  assert.equal(await second, 42);
  assert.equal(gate.current(), null);
});

test('a rejected attempt clears so a later owner can recover', async () => {
  const gate = new SharedAttemptGate<string>();
  const failure = new Error('first provider failed');

  await assert.rejects(gate.run(async () => Promise.reject(failure)), failure);
  assert.equal(gate.current(), null);

  const recovered = gate.run(async () => 'second provider connected');
  assert.equal(await recovered, 'second provider connected');
  assert.equal(gate.current(), null);
});
