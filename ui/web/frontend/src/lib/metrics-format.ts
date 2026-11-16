export const pluralWords = (n: number): string => {
  const rem10 = n % 10
  const rem100 = n % 100
  if (rem100 >= 11 && rem100 <= 19) return 'слов'
  if (rem10 === 1) return 'слово'
  if (rem10 >= 2 && rem10 <= 4) return 'слова'
  return 'слов'
}

export const fmtSpan = (sec: number | null): string => {
  if (sec === null) return 'нет'
  if (sec < 60) return `${sec.toFixed(1).replace('.', ',')} с`
  const total = Math.round(sec)
  const minutes = Math.floor(total / 60)
  const seconds = total % 60
  return `${minutes} мин ${seconds} с`
}

export const fmtMs = (ms: number | null): string => {
  if (ms === null) return 'нет'
  const sec = ms / 1000
  if (sec < 0.1) return 'меньше 0,1 с'
  return `${sec.toFixed(1).replace('.', ',')} с`
}

export const signedNumber = (value: number): string =>
  `${value > 0 ? '+' : ''}${value.toFixed(1).replace('.', ',')}`

export const pct = (value: number): string => `${Math.round(value)} %`

export const clippingText = (ratio: number): string => {
  const value = ratio * 100
  if (value < 0.1) return '<0,1 %'
  return `${value.toFixed(1).replace('.', ',')} %`
}
