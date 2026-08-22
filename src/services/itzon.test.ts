import test from 'node:test';
import assert from 'node:assert/strict';

import { itzonStreamToTwitchStream, partitionItzonStreams } from './itzon.ts';

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
