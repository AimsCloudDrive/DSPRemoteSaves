export function formatBytes(bytes?: number | null): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes)) return '—'
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB', 'PB']
  let value = bytes / 1024
  let index = 0
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024
    index += 1
  }
  return `${value.toFixed(value >= 10 ? 1 : 2)} ${units[index]}`
}

export function formatTime(ms?: number | null): string {
  if (!ms) return '—'
  return new Date(ms).toLocaleString()
}

export function formatDate(ms?: number | null): string {
  if (!ms) return '—'
  return new Date(ms).toLocaleDateString()
}

export function shortHash(hash?: string | null, len = 8): string {
  if (!hash) return '—'
  return hash.slice(0, len)
}

export function parseBytes(input: string): number {
  const text = input.trim().toUpperCase()
  const match = text.match(/^([\d.]+)\s*(B|KB|MB|GB|TB)?$/)
  if (!match) return Number.NaN
  const value = Number.parseFloat(match[1])
  const unit = match[2] || 'B'
  const factor: Record<string, number> = {
    B: 1,
    KB: 1024,
    MB: 1024 ** 2,
    GB: 1024 ** 3,
    TB: 1024 ** 4,
  }
  return Math.round(value * factor[unit])
}

export function humanBytes(bytes: number): string {
  if (!Number.isFinite(bytes)) return ''
  return formatBytes(bytes)
}
