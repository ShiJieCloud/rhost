<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch, type Ref } from 'vue'
import { toast } from '../composables/useToast'
import type { KeyType, SshKey } from '../types'
import {
  addKey,
  askDelete,
  cancelDelete,
  closeKeyModal,
  confirmDelete,
  copyText,
  deletingKeyId,
  detectKeyType,
  detailKeyId,
  findKey,
  formatKeyDate,
  KEY_BITS_MAP,
  KEY_DEFAULT_BITS,
  keyModal,
  keySlug,
  parseListInput,
  randomFingerprint,
  randomPublicKey,
  relKeyTime,
} from '../stores/keys'

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms))

/* ==================== 标签 chip 输入（与新建连接弹窗同款交互） ==================== */
function commitTagsTo(list: Ref<string[]>, draft: Ref<string>) {
  draft.value.split(/[,，]/).map(s => s.trim()).filter(Boolean).forEach(t => {
    if (!list.value.includes(t)) list.value.push(t)
  })
  draft.value = ''
}

function onTagKeydown(list: Ref<string[]>, draft: Ref<string>, e: KeyboardEvent) {
  if (e.key === 'Enter' || e.key === ',' || e.key === '，') {
    e.preventDefault()
    commitTagsTo(list, draft)
  } else if (e.key === 'Backspace' && !draft.value && list.value.length) {
    list.value.pop()
  }
}

/* 模板中 ref 会自动解包，因此暴露各表单专用方法 */
function commitGenTags() {
  commitTagsTo(genTags, genTagDraft)
}
function onGenTagKeydown(e: KeyboardEvent) {
  onTagKeydown(genTags, genTagDraft, e)
}
function removeGenTag(t: string) {
  genTags.value = genTags.value.filter(x => x !== t)
}
function commitImpTags() {
  commitTagsTo(impTags, impTagDraft)
}
function onImpTagKeydown(e: KeyboardEvent) {
  onTagKeydown(impTags, impTagDraft, e)
}
function removeImpTag(t: string) {
  impTags.value = impTags.value.filter(x => x !== t)
}

/* ==================== 生成密钥 ==================== */
const genName = ref('')
const genType = ref<KeyType>('ED25519')
const genBits = ref(KEY_DEFAULT_BITS.ED25519)
const genComment = ref('')
const genHosts = ref('')
const genTags = ref<string[]>([])
const genTagDraft = ref('')
const genPass = ref('')
const genPassShow = ref(false)
const genAutoCopy = ref(true)
const generating = ref(false)
const genProgress = ref(0)
const genStatus = ref('')

function resetGenerate() {
  genName.value = ''
  genType.value = 'ED25519'
  genBits.value = KEY_DEFAULT_BITS.ED25519
  genComment.value = ''
  genHosts.value = ''
  genTags.value = []
  genTagDraft.value = ''
  genPass.value = ''
  genPassShow.value = false
  genAutoCopy.value = true
  generating.value = false
  genProgress.value = 0
  genStatus.value = ''
}

function pickType(t: KeyType) {
  genType.value = t
  const opts = KEY_BITS_MAP[t]
  genBits.value = opts.length ? opts[opts.length - 1]! : KEY_DEFAULT_BITS[t]
}

async function submitGenerate() {
  const name = genName.value.trim()
  if (!name) {
    toast('请填写密钥名称', 'err')
    return
  }

  generating.value = true
  genProgress.value = 0
  const steps: Array<[number, string]> = [
    [18, '正在初始化随机数发生器…'],
    [42, `正在生成 ${genType.value} 密钥对…`],
    [68, '正在计算 SHA256 指纹…'],
    [88, '正在写入 ~/.ssh/ 目录…'],
    [100, '密钥生成完成'],
  ]
  for (const [p, t] of steps) {
    genProgress.value = p
    genStatus.value = t
    await sleep(240)
  }
  await sleep(260)

  const type = genType.value
  const bits = KEY_BITS_MAP[type].length ? genBits.value : KEY_DEFAULT_BITS[type]
  const comment = genComment.value.trim() || 'user@localhost'
  const hostsList = parseListInput(genHosts.value)
  // TODO: 接入 Tauri 后端后改为调用 ssh-keygen 并读取真实公钥/指纹
  const created: Omit<SshKey, 'id'> = {
    name,
    type,
    bits,
    comment,
    fingerprint: randomFingerprint(),
    publicKey: randomPublicKey(type),
    privatePath:
      '~/.ssh/id_' +
      (type === 'ED25519' ? 'ed25519' : type === 'RSA' ? 'rsa' : 'ecdsa') +
      '_' + keySlug(name),
    passphrase: genPass.value.length > 0,
    created: new Date().toISOString(),
    lastUsed: null,
    hosts: hostsList,
    tags: [...genTags.value],
  }
  const item = addKey(created)
  closeKeyModal()

  if (genAutoCopy.value) {
    const ok = await copyText(`${item.publicKey} ${comment}`)
    toast(
      ok
        ? `密钥「${name}」已生成，公钥已复制${hostsList.length ? `，已关联 ${hostsList.length} 台主机` : ''}`
        : `密钥「${name}」已生成（公钥复制失败）`,
      ok ? 'ok' : 'warn',
      2800,
    )
  } else {
    toast(`密钥「${name}」已生成`, 'ok', 2400)
  }
}

/* ==================== 导入密钥 ==================== */
const impName = ref('')
const impHosts = ref('')
const impTags = ref<string[]>([])
const impTagDraft = ref('')
const impText = ref('')
const impFile = ref<HTMLInputElement | null>(null)

function resetImport() {
  impName.value = ''
  impHosts.value = ''
  impTags.value = []
  impTagDraft.value = ''
  impText.value = ''
  if (impFile.value) impFile.value.value = ''
}

function onImpFile(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0]
  if (!file) return
  const reader = new FileReader()
  reader.onload = () => {
    impText.value = String(reader.result ?? '').slice(0, 8000)
    if (!impName.value.trim()) {
      impName.value = file.name.replace(/\.[^.]+$/, '')
    }
    toast(`已读取文件：${file.name}`, 'info', 2200)
  }
  reader.readAsText(file)
}

function submitImport() {
  const text = impText.value.trim()
  if (!text) {
    toast('请粘贴或选择密钥文件', 'err')
    return
  }
  if (!/BEGIN|ssh-(rsa|ed25519|dss)|ecdsa-sha2/i.test(text)) {
    toast('密钥格式无法识别，请检查内容', 'err')
    return
  }

  const type = detectKeyType(text)
  const publicOnly = /^ssh-|^ecdsa-sha2-/m.test(text) && !/BEGIN/.test(text)
  const name = impName.value.trim() || `导入的密钥`

  addKey({
    name,
    type,
    bits: KEY_DEFAULT_BITS[type],
    comment: 'imported',
    fingerprint: randomFingerprint(),
    publicKey: publicOnly ? text.split('\n')[0]!.trim() : randomPublicKey(type),
    privatePath: publicOnly ? null : `~/.ssh/${keySlug(name)}`,
    passphrase: false,
    created: new Date().toISOString(),
    lastUsed: null,
    hosts: parseListInput(impHosts.value),
    tags: [...impTags.value],
  })
  closeKeyModal()
  toast(`已导入密钥「${name}」`, 'ok', 2400)
}

/* ==================== 详情 ==================== */
const detailKey = computed<SshKey | null>(() => findKey(detailKeyId.value))

async function copyInDetail(text: string, label: string) {
  const ok = await copyText(text)
  toast(ok ? `${label}已复制到剪贴板` : '复制失败', ok ? 'ok' : 'err')
}

/* ==================== 删除确认 ==================== */
const deletingKey = computed<SshKey | null>(() => findKey(deletingKeyId.value))

function onConfirmDelete() {
  const removed = confirmDelete()
  if (removed) toast(`已删除密钥「${removed.name}」`, 'info', 2400)
}

/* ==================== 弹窗生命周期 ==================== */
watch(keyModal, async kind => {
  if (kind === 'generate') {
    resetGenerate()
    await nextTick()
    setTimeout(() => document.getElementById('kmGenName')?.focus(), 160)
  } else if (kind === 'import') {
    resetImport()
    await nextTick()
    setTimeout(() => document.getElementById('kmImpText')?.focus(), 160)
  }
})

function onKeydown(e: KeyboardEvent) {
  if (e.key !== 'Escape') return
  if (deletingKeyId.value) {
    cancelDelete()
  } else if (keyModal.value) {
    closeKeyModal()
  }
}

onMounted(() => document.addEventListener('keydown', onKeydown))
onUnmounted(() => document.removeEventListener('keydown', onKeydown))

const TYPE_CLS: Record<KeyType, string> = {
  ED25519: 'ed25519',
  RSA: 'rsa',
  ECDSA: 'ecdsa',
}
</script>

<template>
  <!-- ============ 生成密钥弹窗 ============ -->
  <div
    v-if="keyModal === 'generate'"
    class="mask show"
    @click.self="closeKeyModal"
  >
    <div class="modal km-modal" role="dialog" aria-modal="true">
      <div class="modal-head">
        <div class="modal-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <circle cx="7.5" cy="15.5" r="4.5" />
            <path d="M10.6 12.4L21 2M18 2h3v3" />
          </svg>
        </div>
        <div class="modal-title">
          <h2>生成新的 SSH 密钥</h2>
          <p>密钥将保存在本地 ~/.ssh/ 目录中</p>
        </div>
        <button class="modal-close" title="关闭" @click="closeKeyModal">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div v-if="!generating" class="km-body">
        <div class="field">
          <label>密钥名称<span class="req">*</span></label>
          <input id="kmGenName" v-model="genName" type="text"
                 placeholder="例如：生产服务器、GitHub 部署密钥" spellcheck="false" />
        </div>

        <div class="field">
          <label>密钥类型</label>
          <div class="km-type-grid">
            <button
              v-for="t in (['ED25519', 'RSA', 'ECDSA'] as KeyType[])"
              :key="t"
              type="button"
              class="km-type-opt"
              :class="{ active: genType === t }"
              @click="pickType(t)"
            >
              <span class="km-type-name">{{ t }}</span>
              <span class="km-type-desc">
                {{ t === 'ED25519' ? '推荐 · 更短更快更安全'
                  : t === 'RSA' ? '兼容性最好 · 老服务器适用'
                  : '体积小 · 性能优秀' }}
              </span>
            </button>
          </div>
        </div>

        <div v-if="KEY_BITS_MAP[genType].length" class="field">
          <label>密钥长度</label>
          <select v-model="genBits">
            <option v-for="b in KEY_BITS_MAP[genType]" :key="b" :value="b">{{ b }} 位</option>
          </select>
        </div>

        <div class="field">
          <label>备注（Comment）</label>
          <input v-model="genComment" type="text" placeholder="user@hostname" spellcheck="false" />
        </div>

        <div class="field">
          <label>关联主机（可选）</label>
          <input v-model="genHosts" type="text"
                 placeholder="prod-web-01, prod-web-02, github.com" spellcheck="false" />
          <span class="desc">多个主机用英文逗号分隔，便于后续识别密钥用途与排查连接问题。</span>
        </div>

        <div class="field">
          <label>标签（可选）</label>
          <div class="tags-input" @click="($event.currentTarget as HTMLElement).querySelector('input')?.focus()">
            <span v-for="t in genTags" :key="t" class="tag-chip">
              {{ t }}
              <button type="button" class="tag-chip-x" @click.stop="removeGenTag(t)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"
                     stroke-linecap="round">
                  <line x1="6" y1="6" x2="18" y2="18"></line>
                  <line x1="18" y1="6" x2="6" y2="18"></line>
                </svg>
              </button>
            </span>
            <input
              v-model="genTagDraft"
              type="text"
              :placeholder="genTags.length ? '' : '生产, 部署'"
              spellcheck="false"
              @keydown="onGenTagKeydown($event)"
              @blur="commitGenTags()"
            />
          </div>
          <span class="desc">回车或逗号添加，可在列表页按标签筛选。</span>
        </div>

        <div class="field">
          <label>密码短语（可选）</label>
          <div class="pw-wrap">
            <input
              v-model="genPass"
              :type="genPassShow ? 'text' : 'password'"
              placeholder="留空表示不加密私钥"
              spellcheck="false"
            />
            <button type="button" class="pw-toggle" :class="{ on: genPassShow }"
                    :title="genPassShow ? '隐藏' : '显示'"
                    @click="genPassShow = !genPassShow">
              <svg v-if="genPassShow" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" />
                <line x1="1" y1="1" x2="23" y2="23" />
              </svg>
              <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
                <circle cx="12" cy="12" r="3" />
              </svg>
            </button>
          </div>
          <span class="desc">设置密码短语后，每次使用私钥都需要输入，安全性更高。</span>
        </div>

        <label class="km-check">
          <input v-model="genAutoCopy" type="checkbox" />
          <span>生成后自动复制公钥到剪贴板</span>
        </label>
      </div>

      <div v-else class="km-progress">
        <div class="km-spinner"></div>
        <div class="km-progress-status">{{ genStatus }}</div>
        <div class="km-bar"><div class="km-bar-fill" :style="{ width: genProgress + '%' }"></div></div>
      </div>

      <div v-if="!generating" class="modal-foot">
        <span class="km-hint"><kbd>Esc</kbd> 取消</span>
        <span class="spacer"></span>
        <button type="button" class="btn ghost" @click="closeKeyModal">取消</button>
        <button type="button" class="btn primary" @click="submitGenerate">生成密钥</button>
      </div>
    </div>
  </div>

  <!-- ============ 导入密钥弹窗 ============ -->
  <div
    v-else-if="keyModal === 'import'"
    class="mask show"
    @click.self="closeKeyModal"
  >
    <div class="modal km-modal" role="dialog" aria-modal="true">
      <div class="modal-head">
        <div class="modal-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
            <path d="M7 10l5 5 5-5" />
            <path d="M12 15V3" />
          </svg>
        </div>
        <div class="modal-title">
          <h2>导入已有密钥</h2>
          <p>支持 OpenSSH、PEM 格式的私钥或公钥</p>
        </div>
        <button class="modal-close" title="关闭" @click="closeKeyModal">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div class="km-body">
        <div class="field">
          <label>密钥名称</label>
          <input v-model="impName" type="text" placeholder="例如：公司跳板机" spellcheck="false" />
        </div>

        <div class="field">
          <label>关联主机（可选）</label>
          <input v-model="impHosts" type="text" placeholder="bastion.company.com, 10.0.0.5"
                 spellcheck="false" />
          <span class="desc">多个主机用英文逗号分隔。</span>
        </div>

        <div class="field">
          <label>标签（可选）</label>
          <div class="tags-input" @click="($event.currentTarget as HTMLElement).querySelector('input')?.focus()">
            <span v-for="t in impTags" :key="t" class="tag-chip">
              {{ t }}
              <button type="button" class="tag-chip-x" @click.stop="removeImpTag(t)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"
                     stroke-linecap="round">
                  <line x1="6" y1="6" x2="18" y2="18"></line>
                  <line x1="18" y1="6" x2="6" y2="18"></line>
                </svg>
              </button>
            </span>
            <input
              v-model="impTagDraft"
              type="text"
              :placeholder="impTags.length ? '' : '内网, 跳板机'"
              spellcheck="false"
              @keydown="onImpTagKeydown($event)"
              @blur="commitImpTags()"
            />
          </div>
          <span class="desc">回车或逗号添加，可在列表页按标签筛选。</span>
        </div>

        <div class="field">
          <label>密钥内容<span class="req">*</span></label>
          <textarea
            id="kmImpText"
            v-model="impText"
            class="km-textarea"
            spellcheck="false"
            placeholder="-----BEGIN OPENSSH PRIVATE KEY-----&#10;b3BlbnNzaC1rZXktdjEAAAAABG5vbmU…&#10;-----END OPENSSH PRIVATE KEY-----"
          ></textarea>
          <span class="desc">
            也可以
            <label class="km-link">选择本地文件
              <input ref="impFile" type="file" hidden accept=".pem,.key,.pub,.txt,*"
                     @change="onImpFile" />
            </label>
            ，密钥内容仅保存在本地，不会上传。
          </span>
        </div>
      </div>

      <div class="modal-foot">
        <span class="km-hint"><kbd>Esc</kbd> 取消</span>
        <span class="spacer"></span>
        <button type="button" class="btn ghost" @click="closeKeyModal">取消</button>
        <button type="button" class="btn primary" @click="submitImport">导入密钥</button>
      </div>
    </div>
  </div>

  <!-- ============ 详情弹窗 ============ -->
  <div
    v-else-if="keyModal === 'detail' && detailKey"
    class="mask show"
    @click.self="closeKeyModal"
  >
    <div class="modal km-modal km-detail-modal" role="dialog" aria-modal="true">
      <div class="modal-head">
        <div class="km-avatar lg" :class="TYPE_CLS[detailKey.type]">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <circle cx="7.5" cy="15.5" r="4.5" />
            <path d="M10.6 12.4L21 2M18 2h3v3" />
          </svg>
        </div>
        <div class="modal-title km-detail-title">
          <h2 :title="detailKey.name">{{ detailKey.name }}</h2>
          <p class="km-detail-meta">
            <span class="km-badge" :class="TYPE_CLS[detailKey.type]">
              {{ detailKey.type }} {{ detailKey.bits }}
            </span>
            <span class="km-mini-tag">
              {{ detailKey.hosts.length ? `${detailKey.hosts.length} 台主机` : '未关联主机' }}
            </span>
            <span v-if="detailKey.passphrase" class="km-mini-tag">已加密</span>
          </p>
        </div>
        <button class="modal-close" title="关闭" @click="closeKeyModal">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div class="km-body">
        <!-- 关联主机 -->
        <section class="km-section">
          <div class="km-section-label">
            <span>关联主机</span>
            <span class="km-count">{{ detailKey.hosts.length }}</span>
          </div>
          <div v-if="detailKey.hosts.length" class="km-host-list">
            <div v-for="h in detailKey.hosts" :key="h" class="km-host-row">
              <span class="km-host-dot"></span>
              <span class="km-host-name">{{ h }}</span>
              <button type="button" class="proj-act" title="复制主机名"
                      @click="copyInDetail(h, '主机名')">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <rect x="9" y="9" width="13" height="13" rx="2" />
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                </svg>
              </button>
            </div>
          </div>
          <div v-else class="km-host-list"><div class="km-host-empty">暂未关联任何主机</div></div>
        </section>

        <!-- 标签 -->
        <section v-if="detailKey.tags.length" class="km-section">
          <div class="km-section-label"><span>标签</span></div>
          <div class="km-tag-box">
            <span v-for="t in detailKey.tags" :key="t" class="km-mini-tag">{{ t }}</span>
          </div>
        </section>

        <!-- 指纹 -->
        <section class="km-section">
          <div class="km-section-label"><span>指纹 (SHA256)</span></div>
          <div class="km-copy-field">
            <code>{{ detailKey.fingerprint }}</code>
            <button type="button" class="proj-act" title="复制指纹"
                    @click="copyInDetail(detailKey.fingerprint, '指纹')">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round">
                <rect x="9" y="9" width="13" height="13" rx="2" />
                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
              </svg>
            </button>
          </div>
        </section>

        <!-- 公钥 -->
        <section class="km-section">
          <div class="km-section-label"><span>公钥</span></div>
          <div class="km-copy-field">
            <code>{{ detailKey.publicKey }} {{ detailKey.comment }}</code>
            <button type="button" class="proj-act" title="复制公钥"
                    @click="copyInDetail(`${detailKey.publicKey} ${detailKey.comment}`.trim(), '公钥')">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round">
                <rect x="9" y="9" width="13" height="13" rx="2" />
                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
              </svg>
            </button>
          </div>
        </section>

        <!-- 基本信息 -->
        <section class="km-section">
          <div class="km-section-label"><span>基本信息</span></div>
          <div class="km-info-grid">
            <div class="km-info-item">
              <div class="k">私钥路径</div>
              <div class="v mono">{{ detailKey.privatePath ?? '—（仅公钥）' }}</div>
            </div>
            <div class="km-info-item">
              <div class="k">密码短语</div>
              <div class="v">{{ detailKey.passphrase ? '已设置' : '未设置' }}</div>
            </div>
            <div class="km-info-item">
              <div class="k">创建时间</div>
              <div class="v">{{ formatKeyDate(detailKey.created) }}</div>
            </div>
            <div class="km-info-item">
              <div class="k">最后使用</div>
              <div class="v">{{ relKeyTime(detailKey.lastUsed) }}</div>
            </div>
          </div>
        </section>

        <!-- 备注 -->
        <section class="km-section">
          <div class="km-section-label"><span>备注</span></div>
          <div class="km-comment">{{ detailKey.comment || '—' }}</div>
        </section>
      </div>

      <div class="modal-foot">
        <button type="button" class="btn danger" @click="askDelete(detailKey.id)">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <polyline points="3 6 5 6 21 6"></polyline>
            <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
            <path d="M10 11v6M14 11v6"></path>
            <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
          </svg>
          删除此密钥
        </button>
        <span class="spacer"></span>
        <button type="button" class="btn ghost" @click="closeKeyModal">关闭</button>
      </div>
    </div>
  </div>

  <!-- ============ 删除确认弹窗（覆盖在最上层） ============ -->
  <div
    v-if="deletingKey"
    class="mask show km-mask-del"
    @click.self="cancelDelete"
  >
    <div class="modal km-modal km-modal-sm" role="alertdialog" aria-modal="true">
      <div class="modal-head">
        <div class="modal-icon km-icon-danger">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <polyline points="3 6 5 6 21 6"></polyline>
            <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
            <path d="M10 11v6M14 11v6"></path>
            <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
          </svg>
        </div>
        <div class="modal-title">
          <h2>删除密钥</h2>
          <p>此操作不可撤销</p>
        </div>
        <button class="modal-close" title="关闭" @click="cancelDelete">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div class="km-body">
        <p class="km-del-text">
          确定要删除密钥「<b>{{ deletingKey.name }}</b>」吗？
        </p>
        <div v-if="deletingKey.hosts.length" class="km-del-hosts">
          <div class="km-del-hosts-label">以下主机将无法继续认证</div>
          <div v-for="h in deletingKey.hosts" :key="h" class="km-del-host mono">· {{ h }}</div>
        </div>
        <span class="km-del-note">本地私钥文件不会被删除。</span>
      </div>

      <div class="modal-foot">
        <span class="spacer"></span>
        <button type="button" class="btn ghost" @click="cancelDelete">取消</button>
        <button type="button" class="btn danger" @click="onConfirmDelete">确认删除</button>
      </div>
    </div>
  </div>
</template>
