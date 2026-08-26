import test from 'node:test';
import assert from 'node:assert/strict';

import {
  itzonLatencyProfile,
  itzonProfileColor,
  itzonStreamToTwitchStream,
  partitionItzonStreams,
} from './itzon.ts';

test('matches itzon profile fallback colors', () => {
  assert.equal(itzonProfileColor('arcade'), 'hsl(128, 65%, 68%)');
});

test('matches itzon desktop HLS latency tiers', () => {
  assert.deepEqual(itzonLatencyProfile(40, true), {
    tier: 'near',
    lowLatencyMode: true,
    liveSyncDuration: 2.5,
    liveMaxLatencyDuration: 8,
  });
  assert.deepEqual(itzonLatencyProfile(41, true), {
    tier: 'mid',
    lowLatencyMode: false,
    liveSyncDuration: 5,
    liveMaxLatencyDuration: 12,
  });
  assert.deepEqual(itzonLatencyProfile(40, false), {
    tier: 'mid',
    lowLatencyMode: false,
    liveSyncDuration: 5,
    liveMaxLatencyDuration: 12,
  });
  assert.deepEqual(itzonLatencyProfile(81, true), {
    tier: 'far',
    lowLatencyMode: false,
    liveSyncDuration: 10,
    liveMaxLatencyDuration: 24,
  });
});

test('itzon directory streams are partitioned by normalized follows', () => {
  const streams = [
    itzonStreamToTwitchStream({ username: 'Arcade', viewers: 100 }),
    itzonStreamToTwitchStream({ username: 'Steqian', viewers: 50 }),
  ];

  const result = partitionItzonStreams(streams, [' ARCADE ', 'offline-channel']);

  assert.deepEqual(result.followed.map(stream => stream.user_login), ['arcade']);
  assert.deepEqual(result.recommended.map(stream => stream.user_login), ['steqian']);
  assert.equal(result.followed[0].provider, 'itzon');
  assert.equal(result.followed[0].user_id, 'itzon:arcade');
});
