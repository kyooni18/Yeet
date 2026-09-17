export async function copyTextToClipboard(value: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(value)
      return true
    }
  } catch {
    // Clipboard permission can be unavailable in embedded or restricted browsers.
    // Fall back to the user-gesture-compatible selection path below.
  }

  const activeElement = document.activeElement instanceof HTMLElement ? document.activeElement : null
  const selection = window.getSelection()
  const selectedRanges: Range[] = []
  if (selection) {
    for (let index = 0; index < selection.rangeCount; index += 1) {
      selectedRanges.push(selection.getRangeAt(index).cloneRange())
    }
  }

  const textarea = document.createElement('textarea')
  textarea.value = value
  textarea.setAttribute('readonly', '')
  textarea.style.position = 'fixed'
  textarea.style.left = '-9999px'
  textarea.style.top = '0'
  textarea.style.width = '1px'
  textarea.style.height = '1px'
  textarea.style.opacity = '0'
  textarea.style.pointerEvents = 'none'
  document.body.appendChild(textarea)
  textarea.focus({ preventScroll: true })
  textarea.select()
  textarea.setSelectionRange(0, textarea.value.length)

  let copied = false
  try {
    copied = document.execCommand('copy')
  } catch {
    copied = false
  } finally {
    textarea.remove()
    if (activeElement?.isConnected) activeElement.focus({ preventScroll: true })
    if (selection) {
      try {
        selection.removeAllRanges()
        for (const range of selectedRanges) selection.addRange(range)
      } catch {
        // The original selection can disappear if its DOM changed during the copy.
      }
    }
  }

  return copied
}
