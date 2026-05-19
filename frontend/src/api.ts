export async function apiGet<T>(path: string): Promise<T> {
  const r = await fetch(path)
  if (!r.ok) {
    throw new Error(`${r.status} ${r.statusText}`)
  }
  return r.json() as Promise<T>
}

export async function apiPost<T>(path: string, body: unknown): Promise<T> {
  const r = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!r.ok) {
    const t = await r.text()
    throw new Error(t || `${r.status}`)
  }
  return r.json() as Promise<T>
}

export async function apiDelete<T = { ok: boolean }>(path: string): Promise<T> {
  const r = await fetch(path, { method: 'DELETE' })
  if (!r.ok) {
    const t = await r.text()
    throw new Error(t || `${r.status}`)
  }
  return r.json() as Promise<T>
}
