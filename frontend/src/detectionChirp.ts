/** Short beep for detection alerts; requires user gesture to unlock (see unlockDetectionAudio). */

let ctx: AudioContext | null = null

export function audioContextState(): AudioContextState | 'missing' {
  return ctx?.state ?? 'missing'
}

export async function unlockDetectionAudio(): Promise<void> {
  const Ctx = window.AudioContext || (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
  if (!Ctx) return
  if (!ctx) {
    ctx = new Ctx()
  }
  if (ctx.state === 'suspended') {
    await ctx.resume()
  }
}

export function playDetectionChirp(): void {
  if (!ctx || ctx.state !== 'running') return
  const t0 = ctx.currentTime
  const osc = ctx.createOscillator()
  const g = ctx.createGain()
  osc.type = 'sine'
  osc.frequency.setValueAtTime(880, t0)
  g.gain.setValueAtTime(0.0001, t0)
  g.gain.exponentialRampToValueAtTime(0.11, t0 + 0.02)
  g.gain.exponentialRampToValueAtTime(0.0001, t0 + 0.13)
  osc.connect(g)
  g.connect(ctx.destination)
  osc.start(t0)
  osc.stop(t0 + 0.14)
}
