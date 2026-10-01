<script setup lang="ts">
import { watchEffect } from 'vue'
import SideBar from '../components/SideBar.vue'
import { ENABLED_NAV } from '../config/homeNav'
import { homeView, setHomeView } from '../stores/homeView'

// 持久化的视图若已下线（enabled 变为 false），回退到一键连接页
watchEffect(() => {
  if (!ENABLED_NAV.some(i => i.id === homeView.value)) setHomeView('quick-connect')
})
</script>

<template>
  <div class="body">
    <SideBar />

    <main class="main">
      <!-- v-show 而非 v-if：切换页面时保留各视图内部状态（与工作台保留终端状态的约定一致） -->
      <div
        v-for="item in ENABLED_NAV"
        :key="item.id"
        v-show="homeView === item.id"
        class="home-view"
      >
        <component :is="item.component" v-bind="item.componentProps" />
      </div>
    </main>
  </div>
</template>
