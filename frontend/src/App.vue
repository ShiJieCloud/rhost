<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import TitleBar from './components/TitleBar.vue'
import WbTitleBar from './components/wb/WbTitleBar.vue'
import HomeView from './views/HomeView.vue'
import Workbench from './views/Workbench.vue'
import StatusBar from './components/wb/StatusBar.vue'
import RightIconRail from './components/RightIconRail.vue'
import RightPanel from './components/RightPanel.vue'
import NewConnectionModal from './components/NewConnectionModal.vue'
import SettingsModal from './components/SettingsModal.vue'
import GroupModal from './components/GroupModal.vue'
import KeysModal from './components/KeysModal.vue'
import PasswordModal from './components/PasswordModal.vue'
import HostKeyModal from './components/HostKeyModal.vue'
import ConfirmModal from './components/ConfirmModal.vue'
import AppToast from './components/AppToast.vue'
import AppLockOverlay from './components/AppLockOverlay.vue'
import { appView, restoreSessions, rightPanelMaximized, rightPanelVisible } from './stores/session'
import { loadHosts } from './stores/hosts'
import { savedSettings, showSettings } from './stores/settings'
import { loadAppConfig } from './stores/appConfig'
import { initGlobalErrorReporting, syncLogConfig } from './stores/applog'
import { appLocked, initAppLock, isBlurLockSuppressed, lockApp } from './stores/appLock'
import { appWindow, detectPlatform, isTauri } from './lib/tauri'

// 标题栏跨平台策略：根节点按平台挂 class，CSS 据此切换安全区方向
// - platform-macos：左侧为原生红绿灯预留安全区（Overlay 模式）
// - platform-windows / platform-linux：右侧渲染前端自绘窗口控制按钮
const platformClass = `platform-${detectPlatform()}`

// macOS 原生全屏后红绿灯隐藏（悬停才浮现），左侧安全区预留失效——
// 挂 is-fullscreen class 供 CSS 取消预留，避免 logo 左侧留白
const isFullscreen = ref(false)

onMounted(async () => {
  if (detectPlatform() !== 'macos') return
  const w = await appWindow()
  if (!w) return
  isFullscreen.value = await w.isFullscreen()
  const unlisten = await w.onResized(async () => {
    isFullscreen.value = await w.isFullscreen()
  })
  onBeforeUnmount(unlisten)
})

// 工作台挂载闩锁：一旦进入过工作台就永久保持挂载（v-show 保活终端状态），
// 即使关闭全部会话停留在空态也不卸载
const workbenchMounted = ref(false)
watch(appView, v => { if (v === 'workbench') workbenchMounted.value = true }, { immediate: true })

/* ============================================================
 * 应用锁：闲置 / 失焦检测 + 全局快捷键（app-lock-design.md §6.4）
 * ============================================================ */

/** 最近一次用户活动时刻（锁定期间冻结不更新，解锁瞬间重置） */
let lastActivityAt = Date.now()
let idleTimer: number | undefined
let blurUnlisten: UnlistenFn | null = null
/** watcher 装配代际：setup 是 async（onFocusChanged 返回 Promise），
 *  设置快速连续变化时，过期一代在 await 返回后必须自我了断，
 *  否则其 unlisten 会覆盖新一代的引用造成监听器泄漏与重复回调。 */
let watcherGen = 0

const ACTIVITY_EVENTS: Array<keyof DocumentEventMap> = ['pointerdown', 'keydown', 'wheel']

function onUserActivity() {
  // 锁屏密码框中的输入不刷新活动时刻（解锁成功才重置）
  if (appLocked.value) return
  lastActivityAt = Date.now()
}

function addActivityListeners() {
  for (const ev of ACTIVITY_EVENTS) {
    document.addEventListener(ev, onUserActivity, true)
  }
}
function removeActivityListeners() {
  for (const ev of ACTIVITY_EVENTS) {
    document.removeEventListener(ev, onUserActivity, true)
  }
}

function teardownLockWatchers() {
  removeActivityListeners()
  if (idleTimer !== undefined) {
    window.clearInterval(idleTimer)
    idleTimer = undefined
  }
  if (blurUnlisten) {
    blurUnlisten()
    blurUnlisten = null
  }
}

/** 按当前开关 / 闲置秒数 / 失焦开关装配监听；设置变化时整体重建 */
async function setupLockWatchers() {
  const gen = ++watcherGen
  teardownLockWatchers()
  if (!isTauri || !savedSettings.appLockEnabled) return
  lastActivityAt = Date.now()

  addActivityListeners()

  idleTimer = window.setInterval(() => {
    if (appLocked.value) return
    // 原生文件对话框打开期间收不到 document 活动事件，但用户正在应用内交互，
    // 不计闲置（持续续命），由回焦时的重置兜底
    if (isBlurLockSuppressed()) {
      lastActivityAt = Date.now()
      return
    }
    const idleSec = savedSettings.appLockIdleSec
    if (idleSec > 0 && Date.now() - lastActivityAt >= idleSec * 1000) {
      lockApp('idle')
    }
  }, 1000)

  if (savedSettings.appLockLockOnBlur) {
    const w = await appWindow()
    const un = await w?.onFocusChanged(ev => {
      if (ev.payload === false) {
        // 原生文件对话框（导入/导出/选私钥）弹出期间的假失焦：不锁
        if (isBlurLockSuppressed()) return
        if (!appLocked.value) lockApp('blur')
        return
      }
      // 对话框关闭回焦：用户活动其实发生在应用内模态里，重置计时且不补判
      if (isBlurLockSuppressed()) {
        lastActivityAt = Date.now()
        return
      }
      // 重新聚焦补判：窗口后台期间 WebView 定时器会被节流
      if (appLocked.value) return
      const idleSec = savedSettings.appLockIdleSec
      if (idleSec > 0 && Date.now() - lastActivityAt >= idleSec * 1000) {
        lockApp('idle')
      }
    }) ?? null
    // await 期间设置又变过：本代已过期，立即卸载自己注册的监听，不碰全局引用
    if (gen !== watcherGen) {
      un?.()
      return
    }
    blurUnlisten = un
  }
}

// 开关 / 档位 / 失焦选项任一变化即时重配（document 监听的增删由 setup/teardown 统一负责）
watch(
  () => [
    savedSettings.appLockEnabled,
    savedSettings.appLockIdleSec,
    savedSettings.appLockLockOnBlur,
  ],
  () => {
    void setupLockWatchers()
  },
)
// 解锁成功：活动时刻重新计时，避免「刚解锁又被闲置锁」
watch(appLocked, locked => {
  if (!locked) lastActivityAt = Date.now()
})

// 手动锁屏快捷键：⌘+Shift+L（macOS）/ Ctrl+Shift+L（其他平台）
function onLockHotkey(ev: KeyboardEvent) {
  if (!(ev.shiftKey && (ev.metaKey || ev.ctrlKey))) return
  if (ev.key !== 'L' && ev.key !== 'l') return
  if (!savedSettings.appLockEnabled || appLocked.value || showSettings.value) return
  ev.preventDefault()
  lockApp('manual')
}
document.addEventListener('keydown', onLockHotkey, true)
onBeforeUnmount(() => {
  watcherGen += 1
  teardownLockWatchers()
  document.removeEventListener('keydown', onLockHotkey, true)
})

// Splash 最短展示时长（ms）：设置项 splashDurationMs（200~5000，消费侧夹取防手改配置文件越界），
// 生产构建过快时补齐，避免启动画面一闪而过；呼吸动画不受影响。
// 注意：必须在 await loadAppConfig() hydrate 之后再读取——savedSettings 在此之前是内置默认值，
// 模块同步阶段求值会导致用户设置永远不生效（曾恒为 400ms）。
const SPLASH_MS_MIN = 200
const SPLASH_MS_MAX = 5000
const SPLASH_MS_DEFAULT = 400
function clampSplashMs(v: unknown): number {
  return Math.min(SPLASH_MS_MAX, Math.max(SPLASH_MS_MIN, Number(v) || SPLASH_MS_DEFAULT))
}

// 启动流程：先注册全局错误上报；然后从后端拉取全节配置快照（localStorage 已下线，
// app_config.json 为唯一真相源）并 hydrate 各 store → 下发日志配置（热生效项立即一致；
// logStoragePath 下次重启生效）→ 加载主机配置 → 恢复上次未关闭的会话（热启动）。
// 交接时机：最短展示与启动任务二者取晚完成后撤掉 splash（docs/splash-design.md §5）；
// 其余启动任务挂起不阻塞交接，3s 后强制放行（业务容错自理，见 §6）——
// 该兜底只截断任务等待，不截断用户配置的最短展示时长
onMounted(async () => {
  initGlobalErrorReporting()
  try {
    await loadAppConfig()
  } catch (e) {
    console.error('loadAppConfig 失败，使用内置默认值:', e)
  }
  // 应用锁初态判定（已启用且密码槽位存在 → splash 撤下即锁屏；设计 §6.7）
  await initAppLock()
  void setupLockWatchers()
  // hydrate 完成后启动最短展示计时（本地 IPC 仅数十 ms，对计时起点影响可忽略）
  const minDelay = new Promise(r => setTimeout(r, clampSplashMs(savedSettings.splashDurationMs)))
  const restBoot = (async () => {
    syncLogConfig()
    await loadHosts()
    restoreSessions()
  })()
  await Promise.all([
    minDelay,
    Promise.race([
      restBoot,
      new Promise(r => setTimeout(r, 3000)),
    ]),
  ])
  ;(window as any).__hideSplash?.()
})
</script>

<template>
  <div class="app" :class="[platformClass, { 'is-fullscreen': isFullscreen }]">
    <TitleBar v-if="appView === 'home'" />
    <WbTitleBar v-else />
    <div class="app-body" :class="{ 'rp-maximized': rightPanelVisible && rightPanelMaximized }">
      <HomeView v-show="appView === 'home'" />
      <Workbench v-if="workbenchMounted" v-show="appView === 'workbench'" />
      <RightPanel v-show="rightPanelVisible" />
      <RightIconRail />
    </div>

    <!-- 状态栏提升到 App 层横贯全窗：右侧栏/图标栏底部对齐状态栏上沿，不再覆盖状态栏高度带 -->
    <StatusBar v-if="appView === 'workbench'" />

    <NewConnectionModal />
    <SettingsModal />
    <GroupModal />
    <KeysModal />
    <PasswordModal />
    <HostKeyModal />
    <ConfirmModal />
    <AppToast />

    <!-- 应用锁全屏遮罩：z-index:1000，必须最后挂载盖住一切 -->
    <AppLockOverlay />
  </div>
</template>
