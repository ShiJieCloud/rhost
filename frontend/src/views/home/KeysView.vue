<script setup lang="ts">
import { computed, ref } from 'vue'
import { toast } from '../../composables/useToast'
import type { KeyType, SshKey } from '../../types'
import {
  askDelete,
  copyText,
  keys,
  openDetail,
  openGenerate,
  openImport,
  relKeyTime,
  shortFingerprint,
} from '../../stores/keys'

/* ================= 类型元数据 ================= */
interface TypeMeta {
  cls: string
}

const TYPE_META: Record<KeyType, TypeMeta> = {
  ED25519: { cls: 'ed25519' },
  RSA: { cls: 'rsa' },
  ECDSA: { cls: 'ecdsa' },
}

function metaOf(k: SshKey): TypeMeta {
  return TYPE_META[k.type] ?? TYPE_META.RSA
}

/* ================= 搜索 / 筛选 / 排序 ================= */
const q = ref('')
const typeFilter = ref<'all' | KeyType>('all')
const tagFilter = ref('all')
const sortBy = ref<'created' | 'used' | 'hosts' | 'name'>('created')

const allTags = computed(() => {
  const set = new Set<string>()
  keys.value.forEach(k => k.tags.forEach(t => set.add(t)))
  return Array.from(set).sort((a, b) => a.localeCompare(b, 'zh'))
})

const visibleKeys = computed<SshKey[]>(() => {
  const kw = q.value.trim().toLowerCase()
  let list = keys.value.filter(k => {
    if (typeFilter.value !== 'all' && k.type !== typeFilter.value) return false
    if (tagFilter.value !== 'all' && !k.tags.includes(tagFilter.value)) return false
    if (!kw) return true
    const hay = [
      k.name,
      k.comment,
      k.fingerprint,
      k.tags.join(' '),
      k.hosts.join(' '),
    ].join(' ').toLowerCase()
    return hay.includes(kw)
  })

  list = [...list].sort((a, b) => {
    if (sortBy.value === 'name') return a.name.localeCompare(b.name, 'zh')
    if (sortBy.value === 'used') {
      return new Date(b.lastUsed ?? 0).getTime() - new Date(a.lastUsed ?? 0).getTime()
    }
    if (sortBy.value === 'hosts') return b.hosts.length - a.hosts.length
    return new Date(b.created).getTime() - new Date(a.created).getTime()
  })
  return list
})

const hasFilter = computed(
  () =>
    !!q.value.trim() ||
    typeFilter.value !== 'all' ||
    tagFilter.value !== 'all' ||
    sortBy.value !== 'created',
)

function clearFilter() {
  q.value = ''
  typeFilter.value = 'all'
  tagFilter.value = 'all'
  sortBy.value = 'created'
}

/* ================= 行操作 ================= */
async function copyPubKey(k: SshKey) {
  const ok = await copyText(`${k.publicKey} ${k.comment || ''}`.trim())
  toast(ok ? '公钥已复制到剪贴板' : '复制失败，请手动复制', ok ? 'ok' : 'err')
}
</script>

<template>
  <!-- 顶部工具栏：复用全局 toolbar / search-box / btn 体系 -->
  <div class="toolbar km-toolbar">
    <div class="search-box">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
           stroke-linecap="round" stroke-linejoin="round">
        <circle cx="11" cy="11" r="7"></circle>
        <line x1="21" y1="21" x2="16.7" y2="16.7"></line>
      </svg>
      <input v-model="q" type="text" spellcheck="false"
             placeholder="搜索名称、备注、指纹、标签或关联主机…" />
    </div>

    <select v-model="typeFilter" class="km-select" :class="{ active: typeFilter !== 'all' }">
      <option value="all">全部类型</option>
      <option value="ED25519">ED25519</option>
      <option value="RSA">RSA</option>
      <option value="ECDSA">ECDSA</option>
    </select>

    <select v-model="tagFilter" class="km-select" :class="{ active: tagFilter !== 'all' }">
      <option value="all">全部标签</option>
      <option v-for="t in allTags" :key="t" :value="t">{{ t }}</option>
    </select>

    <select v-model="sortBy" class="km-select">
      <option value="created">按创建时间</option>
      <option value="used">按最后使用</option>
      <option value="hosts">按关联主机数</option>
      <option value="name">按名称</option>
    </select>

    <div class="toolbar-actions">
      <button type="button" class="btn" @click="openImport">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
          <path d="M7 10l5 5 5-5" />
          <path d="M12 15V3" />
        </svg>
        导入密钥
      </button>
      <button type="button" class="btn primary" @click="openGenerate">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
             stroke-linecap="round">
          <path d="M12 5v14M5 12h14" />
        </svg>
        生成新密钥
      </button>
    </div>
  </div>

  <div class="divider"></div>

  <div class="list-meta">
    共 <b>{{ keys.length }}</b> 个密钥
    <template v-if="hasFilter">
      <span class="sep">·</span>
      <span>筛选出 <b>{{ visibleKeys.length }}</b> 个</span>
    </template>
  </div>

  <div class="km-list">
    <div v-if="visibleKeys.length" class="km-card">
      <table class="km-table">
        <thead>
          <tr>
            <th class="col-name">名称</th>
            <th>类型</th>
            <th>指纹</th>
            <th>最后使用</th>
            <th class="col-act">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="k in visibleKeys"
            :key="k.id"
            class="km-row"
            @click="openDetail(k.id)"
          >
            <td>
              <div class="km-key-cell">
                <div class="km-avatar" :class="metaOf(k).cls">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="7.5" cy="15.5" r="4.5" />
                    <path d="M10.6 12.4L21 2M18 2h3v3" />
                  </svg>
                </div>
                <div class="km-key-meta">
                  <div class="km-key-name">{{ k.name }}</div>
                  <div class="km-key-sub">{{ k.comment || '—' }}</div>
                </div>
              </div>
            </td>
            <td>
              <span class="km-badge" :class="metaOf(k).cls">{{ k.type }} {{ k.bits }}</span>
            </td>
            <td>
              <span class="km-fp" :title="k.fingerprint">{{ shortFingerprint(k.fingerprint) }}</span>
            </td>
            <td class="km-dim">{{ relKeyTime(k.lastUsed) }}</td>
            <td>
              <span class="proj-actions km-row-actions">
                <button type="button" class="proj-act" title="复制公钥"
                        @click.stop="copyPubKey(k)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <rect x="9" y="9" width="13" height="13" rx="2" />
                    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                  </svg>
                </button>
                <button type="button" class="proj-act" title="查看详情"
                        @click.stop="openDetail(k.id)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="12" cy="12" r="10" />
                    <path d="M12 16v-4M12 8h.01" />
                  </svg>
                </button>
                <button type="button" class="proj-act del" title="删除"
                        @click.stop="askDelete(k.id)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="3 6 5 6 21 6"></polyline>
                    <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
                    <path d="M10 11v6M14 11v6"></path>
                    <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
                  </svg>
                </button>
              </span>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 空状态：复用全局 empty-state 体系 -->
    <div v-else class="empty-state km-empty">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6"
           stroke-linecap="round" stroke-linejoin="round">
        <circle cx="7.5" cy="15.5" r="4.5" />
        <path d="M10.6 12.4L21 2M18 2h3v3" />
      </svg>
      <p>{{ keys.length ? '没有找到匹配的密钥' : '还没有密钥' }}</p>
      <span v-if="keys.length" class="km-empty-sub">试试调整筛选条件</span>
      <button v-if="hasFilter" type="button" class="btn ghost" @click="clearFilter">清除筛选条件</button>
      <button v-else type="button" class="btn primary" @click="openGenerate">生成新密钥</button>
    </div>
  </div>
</template>
