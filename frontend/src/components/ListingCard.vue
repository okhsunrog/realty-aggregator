<script setup lang="ts">
import { NCard, NTag, NSpace, NProgress, NEllipsis } from 'naive-ui'
import type { Listing } from '@/api'
import { sourceStatusLabel, hasFreshConfirmation, getImageUrl } from '@/api'
import { computed } from 'vue'

const props = defineProps<{
  listing: Listing
  firstImageId?: number | null
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
  return 'error'  // Включая 0
})

// Минимум 5% для отображения красной полоски при 0
const conditionPercentage = computed(() => {
  const c = props.listing.analysis_condition
  if (c === null) return 0
  return c === 0 ? 5 : c
})

const imageUrl = computed(() => {
  if (props.firstImageId) {
    return getImageUrl(props.firstImageId)
  }
  return null
})
</script>

<template>
  <n-card hoverable style="cursor: pointer" @click="emit('click', listing)">
    <template #cover>
      <div class="card-cover">
        <img v-if="imageUrl" :src="imageUrl" :alt="listing.title || 'Listing'" />
        <div v-else class="no-image">Фото пока недоступно</div>
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
      <n-tag size="small" :type="listing.availability_state === 'removed' ? 'error' : hasFreshConfirmation(listing) ? 'success' : 'warning'">
        {{ sourceStatusLabel(listing) }}
      </n-tag>
      <div v-if="listing.actuality_checked_at" style="font-size: 0.8em; color: #888">Сведения от {{ new Date(listing.actuality_checked_at).toLocaleString('ru-RU') }}</div>
      <div v-else-if="listing.source_time" style="font-size: 0.8em; color: #888">Встречалось {{ new Date(listing.source_time).toLocaleString('ru-RU') }}</div>
      <div class="info-row">
        <span v-if="listing.rooms">{{ listing.rooms }} комн.</span>
        <span v-if="listing.area">{{ listing.area }} м²</span>
        <span v-if="listing.floor && listing.floors_total">
          {{ listing.floor }}/{{ listing.floors_total }} эт.
        </span>
      </div>

      <n-ellipsis v-if="listing.address || listing.city" :line-clamp="1" style="color: #888; font-size: 0.9em;">
        {{ [listing.city, listing.address].filter(Boolean).join(', ') }}
      </n-ellipsis>

      <div v-if="listing.analysis_condition !== null" class="condition-row">
        <span>Состояние: {{ listing.analysis_condition }}/100</span>
        <n-progress
          type="line"
          :percentage="conditionPercentage"
          :status="conditionColor"
          :show-indicator="false"
          style="width: 80px"
        />
      </div>

      <n-tag v-if="listing.analysis_condition === null" size="small">
        {{ listing.analysis_analyzed_at ? 'Недостаточно данных' : 'Ожидает анализа' }}
      </n-tag>
      <n-space v-if="listing.analysis_is_shell || listing.analysis_red_flags?.length" wrap size="small">
        <n-tag v-if="listing.analysis_is_shell" type="warning" size="small">Без отделки</n-tag>
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

.condition-row {
  display: flex;
  align-items: center;
  gap: 8px;
  color: #aaa;
  font-size: 0.85em;
}
</style>
