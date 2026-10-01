/** 是否运行在 Tauri 桌面环境（浏览器 dev 模式为 false） */
export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

/** 获取当前窗口实例；浏览器环境返回 null（调用方需判空） */
export async function appWindow() {
  if (!isTauri) return null
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  return getCurrentWindow()
}
