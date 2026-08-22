import assert from 'node:assert/strict';
import test from 'node:test';
import { canSendToProvider } from './providerSendEligibility.ts';

const disconnected = {
  twitchUserId: null,
  kickConnected: false,
  youtubeConnected: false,
  itzonCapability: 'needs_login' as const,
};

test('Twitch requires its own OAuth identity', () => {
  assert.equal(canSendToProvider('twitch', disconnected), false);
  assert.equal(canSendToProvider('twitch', { ...disconnected, twitchUserId: '123' }), true);
});

test('itzon readiness is independent from Twitch login', () => {
  assert.equal(
    canSendToProvider('itzon', { ...disconnected, itzonCapability: 'sendable' }),
    true,
  );
  assert.equal(
    canSendToProvider('itzon', {
      ...disconnected,
      twitchUserId: '123',
      itzonCapability: 'needs_login',
    }),
    false,
  );
});

test('connected provider accounts do not inherit Twitch state', () => {
  assert.equal(canSendToProvider('kick', { ...disconnected, kickConnected: true }), true);
  assert.equal(canSendToProvider('youtube', { ...disconnected, youtubeConnected: true }), true);
});
