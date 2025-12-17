<script setup lang="ts">
import { NCard, NTag, NSpace, NProgress, NEllipsis } from 'naive-ui'
import type { Listing } from '@/api'
import { getImageUrl } from '@/api'
import { computed } from 'vue'

const props = defineProps<{
  listing: Listing
  firstImageKey?: string | null
}>()

const emit = defineEmits<{
  click: [listing: Listing]
}>()

const priceFormatted = computed(() => {
  if (!props.listing.price) return 'Цена не указана'
  return new Intl.NumberFormat('ru-RU', {
    style: 'currency',
    currency: 'RUB',
    maximumFractionDigits: 0,
  }).format(props.listing.price)
})

const conditionColor = computed(() => {
  const c = props.listing.analysis_condition
  if (c === null) return 'default'
  if (c >= 75) return 'success'
  if (c >= 50) return 'info'
  if (c >= 25) return 'warning'
  return 'error'
})

const conditionText = computed(() => {
  const c = props.listing.analysis_condition
  if (c === null) return null
  if (c >= 75) return 'Отличное'
  if (c >= 50) return 'Хорошее'
  if (c >= 25) return 'Требует ремонта'
  return 'Черновая отделка'
})

const imageUrl = computed(() => {
  if (props.firstImageKey) {
    return getImageUrl(props.firstImageKey)
  }
  return null
})
</script>

<template>
  <n-card hoverable style="cursor: pointer" @click="emit('click', listing)">
    <template #cover>
      <div class="card-cover">
        <img v-if="imageUrl" :src="imageUrl" :alt="listing.title || 'Listing'" />
        <div v-else class="no-image">📷 Нет фото</div>
      </div>
    </template>

    <template #header>
      <n-ellipsis :line-clamp="1">
        {{ listing.title || 'Без названия' }}
      </n-ellipsis>
    </template>

    <template #header-extra>
      <span class="price">{{ priceFormatted }}</span>
    </template>

    <n-space vertical size="small">
      <div class="info-row">
        <span v-if="listing.rooms">{{ listing.rooms }} комн.</span>
        <span v-if="listing.area">{{ listing.area }} м²</span>
        <span v-if="listing.floor && listing.floors_total">
          {{ listing.floor }}/{{ listing.floors_total }} эт.
        </span>
      </div>

      <n-ellipsis v-if="listing.address" :line-clamp="1" style="color: #888; font-size: 0.9em;">
        {{ listing.address }}
      </n-ellipsis>

      <n-space v-if="listing.analysis_condition !== null" align="center" size="small">
        <n-progress
          type="line"
          :percentage="listing.analysis_condition"
          :status="conditionColor"
          :show-indicator="false"
          style="width: 80px"
        />
        <n-tag :type="conditionColor" size="small">{{ conditionText }}</n-tag>
      </n-space>

      <n-space v-if="listing.analysis_red_flags?.length" wrap>
        <n-tag v-for="flag in listing.analysis_red_flags" :key="flag" type="error" size="small">
          {{ flag }}
        </n-tag>
      </n-space>
    </n-space>
  </n-card>
</template>

<style scoped>
.card-cover {
  height: 180px;
  overflow: hidden;
  background: #1a1a1a;
}

.card-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.no-image {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #666;
  font-size: 1.2em;
}

.price {
  color: #63e2b7;
  font-weight: 600;
  font-size: 1.1em;
}

.info-row {
  display: flex;
  gap: 12px;
  color: #aaa;
  font-size: 0.9em;
}
</style>
