import type { Component } from 'vue'
import type { HomeViewId } from '../stores/homeView'
import HostsView from '../views/home/HostsView.vue'
import QuickConnectView from '../views/home/QuickConnectView.vue'
import GroupsView from '../views/home/GroupsView.vue'
import KeysView from '../views/home/KeysView.vue'
import { keys } from '../stores/keys'
import HomePlaceholder from '../views/home/HomePlaceholder.vue'

/**
 * 首页左侧菜单的唯一数据源。
 * 新增页面流程：
 *   1. 在 stores/homeView.ts 扩展 HomeViewId 联合类型
 *   2. 在 views/home/ 下新建页面组件
 *   3. 在本数组加一项并把 enabled 置为 true
 * 未上线的功能保持 enabled: false（侧栏渲染为置灰禁用态，不可点击）。
 */
export interface HomeNavItem {
  id: HomeViewId
  label: string
  icon: string
  /** false = 功能未上线，置灰且不可切换 */
  enabled: boolean
  /** 徽标：返回字符串则展示（如密钥数量、未读日志数），null/无值不展示 */
  badge?: () => string | null
  /** 主区渲染的页面组件（enabled 为 true 时生效） */
  component: Component
  /** 透传给页面组件的 props（占位页等场景使用） */
  componentProps?: Record<string, unknown>
}

export const HOME_NAV: HomeNavItem[] = [
  {
    id: 'quick-connect',
    label: '一键连接',
    enabled: true,
    component: QuickConnectView,
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>',
  },
  {
    id: 'hosts',
    label: '主机',
    enabled: true,
    component: HostsView,
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="6" rx="2"/><rect x="3" y="14" width="18" height="6" rx="2"/><line x1="7" y1="7" x2="7.01" y2="7"/><line x1="7" y1="17" x2="7.01" y2="17"/></svg>',
  },
  {
    id: 'groups',
    label: '分组',
    enabled: true,
    component: GroupsView,
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>',
  },
  {
    id: 'keys',
    label: '密钥',
    enabled: true,
    component: KeysView,
    badge: () => (keys.value.length ? String(keys.value.length) : null),
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 2l-2 2m-7.6 7.6a5 5 0 1 1-7.1 7.1 5 5 0 0 1 7.1-7.1zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3"/></svg>',
  },
  {
    id: 'logs',
    label: '会话日志',
    enabled: false,
    component: HomePlaceholder,
    componentProps: { title: '会话日志', desc: '查看历史会话与命令记录，功能开发中' },
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10 9 9 9 8 9"/></svg>',
  },
]

/** 可切换的视图（供主区 v-show 渲染） */
export const ENABLED_NAV = HOME_NAV.filter(i => i.enabled)
