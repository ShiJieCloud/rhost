/** 是否运行在 Tauri 桌面环境（浏览器 dev 模式为 false） */
export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

export type AppPlatform = 'macos' | 'windows' | 'linux' | 'unknown'

/**
 * 同步探测运行平台。优先读 lib.rs 通过 initialization_script 写入的
 * window.__RHOST_BOOT__.platform（任意前端脚本执行前即就绪，与首帧渲染无竞态）；
 * 浏览器 dev 模式回退到 navigator.userAgent 判定。
 *
 * 用于：标题栏安全区方向（macOS 左侧为原生红绿灯预留）、
 * 是否渲染前端自绘窗口控制按钮（Windows/Linux 右侧）。
 */
export function detectPlatform(): AppPlatform {
  const boot = (window as unknown as { __RHOST_BOOT__?: { platform?: string } }).__RHOST_BOOT__
  const p = boot?.platform
  if (p === 'macos' || p === 'windows' || p === 'linux') return p
  const ua = navigator.userAgent.toLowerCase()
  if (ua.includes('mac')) return 'macos'
  if (ua.includes('windows')) return 'windows'
  if (ua.includes('linux')) return 'linux'
  return 'unknown'
}

/** 获取当前窗口实例；浏览器环境返回 null（调用方需判空） */
export async function appWindow() {
  if (!isTauri) return null
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  return getCurrentWindow()
}
