import { createApp } from 'vue'
import './style.css'
import App from './App.vue'

// Splash JS 层兜底：任何加载期错误立即撤掉启动画面（docs/splash-design.md §6）
window.addEventListener('error', () => (window as any).__hideSplash?.())
window.addEventListener('unhandledrejection', () => (window as any).__hideSplash?.())

createApp(App).mount('#app')
