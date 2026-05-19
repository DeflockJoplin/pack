export function macsToText(v: string[]) {
  return v.join('\n')
}

export function textToMacs(s: string) {
  return s
    .split(/\r?\n/)
    .map((x) => x.trim().toLowerCase())
    .filter(Boolean)
}

export function ssidsToText(v: string[]) {
  return v.join('\n')
}

export function textToSsids(s: string) {
  return s
    .split(/\r?\n/)
    .map((x) => x.trim())
    .filter(Boolean)
}
