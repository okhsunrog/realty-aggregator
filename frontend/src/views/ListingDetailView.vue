<script setup lang="ts">
import { onMounted, ref, computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  NCard,
  NSpace,
  NButton,
  NTag,
  NProgress,
  NSpin,
  NGrid,
  NGi,
  NImage,
  NImageGroup,
  NDivider,
  NDescriptions,
  NDescriptionsItem,
  NAlert,
  NIcon,
  NBreadcrumb,
  NBreadcrumbItem,
} from 'naive-ui'
import { fetchListing, fetchListingImages, getImageUrl, type Listing, type ListingImage } from '@/api'

const route = useRoute()
const router = useRouter()

const listing = ref<Listing | null>(null)
const images = ref<ListingImage[]>([])
const loading = ref(true)
const refreshingImages = ref(false)
const refreshingListing = ref(false)

const listingId = computed(() => Number(route.params.id))

const priceFormatted = computed(() => {
  if (!listing.value?.price) return 'Цена не указана'
  return new Intl.NumberFormat('ru-RU', {
    style: 'currency',
    currency: 'RUB',
    maximumFractionDigits: 0,
  }).format(listing.value.price)
})

const conditionColor = computed(() => {
  const c = listing.value?.analysis_condition
  if (c === null || c === undefined) return 'default'
  if (c === 0) return 'default'  // Нет данных для оценки
  if (c >= 75) return 'success'
  if (c >= 50) return 'info'
  if (c >= 25) return 'warning'
  return 'error'
})

const conditionText = computed(() => {
  const c = listing.value?.analysis_condition
  if (c === null || c === undefined) return 'Не проанализировано'
  if (c === 0) return 'Нет данных'  // Нет фото интерьера
  if (c >= 75) return 'Отличное'
  if (c >= 50) return 'Хорошее'
  if (c >= 25) return 'Требует ремонта'
  return 'Плохое'
})

const downloadedImages = computed(() =>
  images.value.filter(img => img.s3_key && img.status.toLowerCase() === 'downloaded')
)

const analyzedImagesCount = computed(() =>
  images.value.filter(img => img.ai_condition !== null).length
)

async function loadData() {
  loading.value = true
  try {
    const [listingData, imagesData] = await Promise.all([
      fetchListing(listingId.value),
      fetchListingImages(listingId.value),
    ])
    listing.value = listingData
    images.value = imagesData
  } catch (e) {
    console.error('Failed to load listing:', e)
  } finally {
    loading.value = false
  }
}

async function refreshImagesAI() {
  refreshingImages.value = true
  try {
    await fetch(`http://localhost:3000/api/listings/${listingId.value}/analyze/images`, {
      method: 'POST',
    })
    // Wait a bit and reload
    setTimeout(async () => {
      images.value = await fetchListingImages(listingId.value)
      refreshingImages.value = false
    }, 3000)
  } catch (e) {
    console.error('Failed to refresh images AI:', e)
    refreshingImages.value = false
  }
}

async function refreshListingAI() {
  refreshingListing.value = true
  try {
    await fetch(`http://localhost:3000/api/listings/${listingId.value}/analyze`, {
      method: 'POST',
    })
    // Wait a bit and reload
    setTimeout(async () => {
      listing.value = await fetchListing(listingId.value)
      refreshingListing.value = false
    }, 3000)
  } catch (e) {
    console.error('Failed to refresh listing AI:', e)
    refreshingListing.value = false
  }
}

function openOriginal() {
  if (listing.value?.url) {
    window.open(listing.value.url, '_blank')
  }
}

function getImageConditionColor(condition: number | null): string {
  if (condition === null) return 'default'
  if (condition >= 75) return 'success'
  if (condition >= 50) return 'info'
  if (condition >= 25) return 'warning'
  return 'error'
}

onMounted(loadData)
</script>

<template>
  <n-spin :show="loading">
    <n-space vertical size="large" v-if="listing">
      <!-- Breadcrumb -->
      <n-breadcrumb>
        <n-breadcrumb-item @click="router.push('/')">Объявления</n-breadcrumb-item>
        <n-breadcrumb-item>{{ listing.title || 'Без названия' }}</n-breadcrumb-item>
      </n-breadcrumb>

      <!-- Header -->
      <n-card>
        <template #header>
          <n-space justify="space-between" align="center" style="width: 100%">
            <span style="font-size: 1.3em">{{ listing.title || 'Без названия' }}</span>
            <span style="color: #63e2b7; font-size: 1.4em; font-weight: 600">{{ priceFormatted }}</span>
          </n-space>
        </template>

        <n-descriptions :column="3" label-placement="left" bordered>
          <n-descriptions-item label="Адрес">{{ listing.address || '—' }}</n-descriptions-item>
          <n-descriptions-item label="Город">{{ listing.city }}</n-descriptions-item>
          <n-descriptions-item label="Источник">{{ listing.source }}</n-descriptions-item>
          <n-descriptions-item label="Комнат">{{ listing.rooms || '—' }}</n-descriptions-item>
          <n-descriptions-item label="Площадь">{{ listing.area ? `${listing.area} м²` : '—' }}</n-descriptions-item>
          <n-descriptions-item label="Этаж">
            {{ listing.floor && listing.floors_total ? `${listing.floor}/${listing.floors_total}` : '—' }}
          </n-descriptions-item>
        </n-descriptions>

        <template #action>
          <n-button type="primary" @click="openOriginal">Открыть оригинал</n-button>
        </template>
      </n-card>

      <!-- AI Analysis -->
      <n-card title="AI Анализ объявления">
        <template #header-extra>
          <n-button
            :loading="refreshingListing"
            @click="refreshListingAI"
            :disabled="analyzedImagesCount === 0"
          >
            Обновить анализ
          </n-button>
        </template>

        <n-space vertical size="medium">
          <n-space align="center" size="large">
            <n-space align="center">
              <span>Состояние:</span>
              <n-progress
                type="line"
                :percentage="listing.analysis_condition ?? 0"
                :status="conditionColor"
                :show-indicator="true"
                style="width: 150px"
              />
              <n-tag :type="conditionColor">{{ conditionText }}</n-tag>
            </n-space>

            <n-tag v-if="listing.analysis_is_shell" type="warning">Без отделки</n-tag>
            <n-tag v-if="listing.analysis_move_in_ready" type="success">Готово к заселению</n-tag>
          </n-space>

          <n-alert v-if="listing.analysis_red_flags?.length" type="error" title="Красные флаги">
            <n-space>
              <n-tag v-for="flag in listing.analysis_red_flags" :key="flag" type="error">{{ flag }}</n-tag>
            </n-space>
          </n-alert>

          <div v-if="listing.analysis_summary">
            <strong>Резюме:</strong>
            <p style="white-space: pre-wrap; margin-top: 8px">{{ listing.analysis_summary }}</p>
          </div>

          <n-alert v-if="listing.analysis_condition === null" type="info">
            Анализ объявления еще не выполнен. Сначала проанализируйте изображения, затем нажмите "Обновить анализ".
          </n-alert>
        </n-space>
      </n-card>

      <!-- Images Gallery -->
      <n-card title="Изображения">
        <template #header-extra>
          <n-space align="center">
            <span style="color: #888">{{ analyzedImagesCount }}/{{ downloadedImages.length }} проанализировано</span>
            <n-button :loading="refreshingImages" @click="refreshImagesAI">
              Анализировать изображения
            </n-button>
          </n-space>
        </template>

        <n-grid :cols="3" :x-gap="16" :y-gap="16" responsive="screen" :cols-xs="1" :cols-s="2" :cols-m="3">
          <n-gi v-for="image in downloadedImages" :key="image.id">
            <n-card size="small">
              <template #cover>
                <n-image
                  :src="getImageUrl(image.s3_key!)"
                  :alt="`Image ${image.image_index}`"
                  object-fit="cover"
                  height="200"
                  width="100%"
                  lazy
                />
              </template>

              <n-space vertical size="small">
                <n-space v-if="image.ai_condition !== null" align="center">
                  <n-progress
                    type="line"
                    :percentage="image.ai_condition"
                    :status="getImageConditionColor(image.ai_condition)"
                    :show-indicator="false"
                    style="width: 80px"
                  />
                  <span>{{ image.ai_condition }}%</span>
                </n-space>

                <n-space v-if="image.ai_room || image.ai_image_type" wrap>
                  <n-tag v-if="image.ai_room" size="small" type="info">{{ image.ai_room }}</n-tag>
                  <n-tag v-if="image.ai_image_type" size="small">{{ image.ai_image_type }}</n-tag>
                </n-space>

                <div v-if="image.ai_description" style="font-size: 0.85em; color: #aaa">
                  {{ image.ai_description }}
                </div>

                <n-space v-if="image.ai_issues?.length" wrap>
                  <n-tag v-for="issue in image.ai_issues" :key="issue" size="small" type="warning">
                    {{ issue }}
                  </n-tag>
                </n-space>

                <div v-if="!image.ai_condition" style="color: #666; font-size: 0.85em">
                  Не проанализировано
                </div>
              </n-space>
            </n-card>
          </n-gi>
        </n-grid>

        <n-alert v-if="downloadedImages.length === 0" type="info">
          Нет загруженных изображений для этого объявления
        </n-alert>
      </n-card>

      <!-- Description -->
      <n-card v-if="listing.description" title="Описание">
        <p style="white-space: pre-wrap">{{ listing.description }}</p>
      </n-card>
    </n-space>
  </n-spin>
</template>
