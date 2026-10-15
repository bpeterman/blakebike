/**
 * A short sine beep for on-the-bike cues, made with Web Audio so there is no
 * sound file to ship. One context is shared and created lazily; a webview
 * that has no Web Audio (or tests under jsdom) just stays quiet.
 */
let context: AudioContext | null = null;

const audioContext = (): AudioContext | null => {
  if (typeof window === "undefined" || typeof window.AudioContext !== "function") return null;
  context ??= new window.AudioContext();
  // A context made outside a click starts suspended; resuming is harmless otherwise.
  if (context.state === "suspended") void context.resume();
  return context;
};

/** Wake the shared context from a click, so later cues are allowed to play. */
export const primeBeep = (): void => {
  audioContext();
};

export const playBeep = (frequencyHz: number, durationMs = 180): void => {
  const audio = audioContext();
  if (audio === null) return;
  const oscillator = audio.createOscillator();
  const gain = audio.createGain();
  const start = audio.currentTime;
  const end = start + durationMs / 1000;
  oscillator.type = "sine";
  oscillator.frequency.value = frequencyHz;
  // A quick fade in and out so the beep does not click.
  gain.gain.setValueAtTime(0.0001, start);
  gain.gain.exponentialRampToValueAtTime(0.4, start + 0.01);
  gain.gain.exponentialRampToValueAtTime(0.0001, end);
  oscillator.connect(gain).connect(audio.destination);
  oscillator.start(start);
  oscillator.stop(end);
};
