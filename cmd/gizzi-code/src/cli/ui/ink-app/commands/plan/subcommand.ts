/** `/plan view` shows the saved plan without entering plan mode (was /view-plan). */
export function isPlanView(args: string | undefined): boolean {
  const word = (args ?? '').trim().toLowerCase()
  return word === 'view' || word === 'show'
}
