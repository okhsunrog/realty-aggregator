<script setup lang="ts">
import { NConfigProvider, NMessageProvider, NLayout, NLayoutHeader, NLayoutContent, darkTheme } from 'naive-ui'
import { ref } from 'vue'
import { RouterView } from 'vue-router'

const isDark = ref(true)
</script>

<template>
  <n-config-provider :theme="isDark ? darkTheme : undefined">
    <n-message-provider>
      <n-layout style="min-height: 100vh">
        <n-layout-header bordered style="padding: 16px 24px; display: flex; align-items: center; justify-content: space-between;">
          <h1 style="margin: 0; font-size: 1.5rem;">🏠 Недвижимость КБР</h1>
        </n-layout-header>
        <n-layout-content style="padding: 24px;">
          <RouterView />
        </n-layout-content>
      </n-layout>
    </n-message-provider>
  </n-config-provider>
</template>

<style>
body {
  margin: 0;
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
}
</style>
