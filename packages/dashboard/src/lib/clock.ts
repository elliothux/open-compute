export interface Clock {
  now(): number;
}

export const systemClock: Clock = {
  now: () => Date.now(),
};
