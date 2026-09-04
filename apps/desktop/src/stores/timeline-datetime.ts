// Convert a datetime-local filter value into the UTC ISO string the
// timeline query contract compares lexicographically.
export function datetimeLocalToUtcIso(value: string): string | undefined {
  if (!value) {
    return undefined
  }
  const instant = new Date(value)
  if (Number.isNaN(instant.getTime())) {
    return undefined
  }
  return instant.toISOString()
}
