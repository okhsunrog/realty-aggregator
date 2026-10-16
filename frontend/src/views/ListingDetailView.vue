<script setup lang="ts">
import { onUnmounted, watch, ref, computed } from 'vue'
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
  NInput,
  NTooltip,
  NInputNumber,
  NSelect,
  NSwitch,
  useMessage,
} from 'naive-ui'
import { fetchSettings, sourceOriginLabel, imageTypeLabel, roomLabel, sourceStatusLabel, startSourceCheck, fetchReview, saveReview, fetchPriceHistory, startAnalysis, getJobStatus, type ListingReview, type PriceObservation, fetchListing, fetchListingImages, getImageUrl, markListingInactive, type Listing, type ListingImage } from '@/api'

const message = useMessage()
const error = ref<string | null>(null)
const review = ref<ListingReview>({ favorite: false, hidden: false, stage: 'new', notes: '' })
const priceHistory = ref<PriceObservation[]>([])
const savingReview = ref(false)
const certificate = ref<number | null>(Number(localStorage.getItem('certificateEstimate')) || 0)
watch(certificate, value => localStorage.setItem('certificateEstimate', String(value ?? '')))
const stageOptions = [
  { label: 'Новый вариант', value: 'new' }, { label: 'Связаться', value: 'contact' },
  { label: 'Просмотр', value: 'visit' }, { label: 'Проверка документов', value: 'documents' },
  { label: 'Подходит', value: 'approved' }, { label: 'Отклонено', value: 'rejected' },
]
let requestVersion = 0
let disposed = false
let analysisController: AbortController | undefined
const route = useRoute()
const router = useRouter()

const listing = ref<Listing | null>(null)
const images = ref<ListingImage[]>([])
const loading = ref(true)
const checkingSource = ref(false)
const refreshingImages = ref(false)
const refreshingListing = ref(false)
const markingInactive = ref(false)
const customModel = ref('')
const modelOptions = ref<{label: string; value: string}[]>([])
fetchSettings().then(settings => { modelOptions.value = settings.models.map(value => ({label: value, value})); customModel.value = settings.default_model }).catch(() => { /* Default model stays selected on the server. */ })

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
  if (c >= 75) return 'success'
  if (c >= 50) return 'info'
  if (c >= 25) return 'warning'
  return 'error'  // Включая 0 (нет данных для оценки)
})

// Минимум 5% для отображения красной полоски при 0
const conditionPercentage = computed(() => {
  const c = listing.value?.analysis_condition
  if (c === null || c === undefined) return 0
  return c === 0 ? 5 : c
})

const conditionText = computed(() => {
  const c = listing.value?.analysis_condition
  if (c === null || c === undefined) return listing.value?.analysis_analyzed_at ? 'Недостаточно данных' : 'Ожидает анализа'
  if (c === 0) return 'Без отделки'
  if (c >= 85) return 'Отличная отделка'
  if (c >= 65) return 'Хорошая отделка'
  if (c >= 40) return 'Отделка с износом'
  if (c >= 25) return 'Нужен ремонт'
  return 'Плохое'
})

const downloadedImages = computed(() =>
  images.value.filter(img => img.s3_key && img.status.toLowerCase() === 'downloaded')
)

const analyzedImagesCount = computed(() =>
  images.value.filter(img => img.ai_analyzed_at !== null).length
)

async function loadData() {
  const version = ++requestVersion
  analysisController?.abort()
  checkingSource.value = false
  refreshingImages.value = false
  refreshingListing.value = false
  const id = listingId.value
  loading.value = true
  error.value = null
  listing.value = null
  images.value = []
  try {
    const [listingData, imagesData, savedReview, history] = await Promise.all([
      fetchListing(id), fetchListingImages(id), fetchReview(id), fetchPriceHistory(id),
    ])
    if (disposed || version !== requestVersion) return
    listing.value = listingData; images.value = imagesData; review.value = savedReview; priceHistory.value = history
  } catch (e) {
    if (version === requestVersion && !disposed) error.value = e instanceof Error ? e.message : 'Не удалось загрузить квартиру'
  } finally {
    if (version === requestVersion && !disposed) loading.value = false
  }
}

async function refreshAnalysis(photoAnalysis: boolean, sourceCheck = false) {
  const id = listingId.value
  analysisController?.abort()
  const controller = new AbortController()
  analysisController = controller
  const busy = sourceCheck ? checkingSource : photoAnalysis ? refreshingImages : refreshingListing
  busy.value = true
  try {
    if (sourceCheck) await startSourceCheck(id, controller.signal)
    else await startAnalysis(id, photoAnalysis, customModel.value.trim() || undefined, controller.signal)
    for (let attempt = 0; attempt < 600; attempt++) {
      await new Promise<void>((resolve, reject) => {
        if (controller.signal.aborted) { reject(new DOMException('Aborted', 'AbortError')); return }
        const abort = () => { clearTimeout(timer); reject(new DOMException('Aborted', 'AbortError')) }
        const timer = setTimeout(() => { controller.signal.removeEventListener('abort', abort); resolve() }, 1000)
        controller.signal.addEventListener('abort', abort, { once: true })
      })
      const status = await getJobStatus()
      if (controller.signal.aborted) return
      if (status.phase === 'idle') {
        const [updatedListing, updatedImages] = await Promise.all([fetchListing(id), fetchListingImages(id)])
        if (controller.signal.aborted) return
        listing.value = updatedListing; images.value = updatedImages
        if (status.last_error) throw new Error(status.last_error)
        message.success(sourceCheck ? 'Проверка завершена' : 'Анализ завершён')
        return
      }
    }
    message.warning('Задача ещё выполняется. Её статус доступен в списке объявлений.')
  } catch (e) {
    if (!controller.signal.aborted) message.error(e instanceof Error ? e.message : 'Ошибка анализа')
  } finally {
    if (!controller.signal.aborted) busy.value = false
  }
}
function refreshImagesAI() { return refreshAnalysis(true) }
function refreshListingAI() { return refreshAnalysis(false) }
async function saveSelection() {
  savingReview.value = true
  const id = listingId.value
  try {
    await saveReview(id, { ...review.value })
    if (listingId.value === id && !disposed) message.success('Сохранено')
  } catch { message.error('Не удалось сохранить заметки') }
  finally { savingReview.value = false }
}
function formatMoney(value: number) { return new Intl.NumberFormat('ru-RU', { maximumFractionDigits: 0 }).format(value) + ' ₽' }

function openOriginal() {
  if (listing.value?.url) {
    window.open(listing.value.url, '_blank', 'noopener,noreferrer')
  }
}

async function handleMarkInactive() {
  if (!listing.value) return

  markingInactive.value = true
  try {
    const id = listing.value.id
    await markListingInactive(id)
    if (listing.value?.id === id) listing.value.is_active = false
  } catch (e) {
    message.error(e instanceof Error ? e.message : 'Не удалось изменить статус')
  } finally {
    markingInactive.value = false
  }
}

function getImageConditionColor(condition: number | null): 'default' | 'success' | 'info' | 'warning' | 'error' {
  if (condition === null) return 'default'
  if (condition >= 75) return 'success'
  if (condition >= 50) return 'info'
  if (condition >= 25) return 'warning'
  return 'error'
}

watch(listingId, loadData, { immediate: true })
onUnmounted(() => { disposed = true; requestVersion++; analysisController?.abort() })
</script>

<template>
  <n-alert v-if="error" type="error">{{ error }}</n-alert>
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
          <n-space>
            <n-button type="primary" @click="openOriginal">Открыть оригинал</n-button>
            <n-button
              v-if="listing.is_active"
              type="error"
              :loading="markingInactive"
              @click="handleMarkInactive"
            >
              Отметить как неактивное
            </n-button>
            <n-tag v-else type="error">Неактивно</n-tag>
          </n-space>
        </template>
      </n-card>

      <n-card title="Сертификат и доплата">
        <n-space vertical>
          <div>Ориентировочная сумма сертификата, ₽</div>
          <n-input-number v-model:value="certificate" :min="0" :step="10000" style="max-width: 300px" />
          <n-alert type="info">0 ₽ — сумма, которую называли год назад. Укажите подтверждённую сумму перед выбором квартиры.</n-alert>
          <div v-if="certificate !== null && listing.price !== null">
            <strong v-if="listing.price > certificate">Доплата: {{ formatMoney(listing.price - certificate) }}</strong>
            <strong v-else>Неиспользованная часть сертификата: {{ formatMoney(certificate - listing.price) }}</strong>
          </div>
        </n-space>
      </n-card>
      <n-card title="Мой выбор">
        <n-space vertical>
          <n-space align="center"><n-switch v-model:value="review.favorite" /><span>В избранном</span><n-switch v-model:value="review.hidden" /><span>Скрыть из подбора</span></n-space>
          <n-select v-model:value="review.stage" :options="stageOptions" style="max-width: 300px" />
          <n-input v-model:value="review.notes" type="textarea" placeholder="Звонок продавцу, документы, впечатления от просмотра" :maxlength="20000" />
          <n-button :loading="savingReview" @click="saveSelection">Сохранить</n-button>
        </n-space>
      </n-card>
      <n-card v-if="priceHistory.length" title="История цены">
        <div v-for="observation in priceHistory" :key="observation.observed_at">
          {{ new Date(observation.observed_at).toLocaleString('ru-RU') }} · {{ formatMoney(observation.price) }}
        </div>
      </n-card>
      <n-card title="Актуальность объявления">
        <n-space vertical>
          <n-tag :type="!listing.is_active ? 'error' : listing.source_check_status === 'active' ? 'success' : 'warning'">{{ sourceStatusLabel(listing) }}</n-tag>
          <div v-if="listing.source_checked_at">Последняя попытка: {{ new Date(listing.source_checked_at).toLocaleString('ru-RU') }}</div>
          <div v-if="listing.source_check_reason">{{ listing.source_check_reason }}<span v-if="listing.source_check_http_status"> (HTTP {{ listing.source_check_http_status }})</span></div>
          <div v-if="listing.actuality_checked_at">Последняя определённая отметка: {{ new Date(listing.actuality_checked_at).toLocaleString('ru-RU') }} · {{ sourceOriginLabel(listing.actuality_origin) }}</div>
          <n-alert type="info">Автоматическая проверка может встретить капчу или ограничение доступа. Неопределённый результат не подтверждает снятие квартиры с продажи.</n-alert>
          <n-button :loading="checkingSource" :disabled="refreshingImages || refreshingListing" @click="refreshAnalysis(false, true)">Проверить исходную карточку</n-button>
        </n-space>
      </n-card>
      <!-- AI Analysis -->
      <n-card title="AI Анализ объявления">
        <template #header-extra>
          <n-space align="center">
            <n-select
              v-model:value="customModel"
              :options="modelOptions"
              placeholder="Модель анализа"
              style="width: 320px"
              size="small"
            />
            <n-button
              :loading="refreshingListing"
              @click="refreshListingAI"
              :disabled="refreshingImages || checkingSource"
            >
              Обновить анализ
            </n-button>
          </n-space>
        </template>

        <n-space vertical size="medium">
          <n-space align="center" size="large">
            <n-space align="center">
              <span>Состояние:</span>
              <n-progress
                type="line"
                :percentage="conditionPercentage"
                :status="conditionColor"
                :show-indicator="true"
                style="width: 150px"
              />
              <n-tag :type="conditionColor">{{ conditionText }}</n-tag>
            </n-space>

            <n-tag v-if="listing.analysis_is_shell" type="warning">Без отделки</n-tag>
            <n-tag v-if="listing.analysis_move_in_ready" type="success">По анализу: можно заселиться</n-tag>
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

          <div v-if="listing.analysis_model && listing.analysis_model !== 'none'" style="color: #666; font-size: 0.85em; margin-top: 8px">
            Модель: {{ listing.analysis_model }}
          </div>

          <n-alert v-if="listing.analysis_condition === null" type="info">
            {{ listing.analysis_analyzed_at ? 'Недостаточно данных для оценки. Уточните состояние квартиры у продавца.' : 'Анализ объявления ещё не выполнен.' }}
          </n-alert>
        </n-space>
      </n-card>

      <!-- Images Gallery -->
      <n-card title="Изображения">
        <template #header-extra>
          <n-space align="center">
            <span style="color: #888">{{ analyzedImagesCount }}/{{ downloadedImages.length }} проанализировано</span>
            <n-button :loading="refreshingImages" :disabled="refreshingListing || checkingSource" @click="refreshImagesAI">
              Анализировать изображения
            </n-button>
          </n-space>
        </template>

        <n-grid cols="1 s:2 m:3" :x-gap="16" :y-gap="16" responsive="screen">
          <n-gi v-for="image in downloadedImages" :key="image.id">
            <n-card size="small">
              <template #cover>
                <n-image
                  :src="getImageUrl(image.id)"
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
                  <span>{{ image.ai_condition }}/100</span>
                </n-space>

                <n-space v-if="image.ai_room || image.ai_image_type" wrap>
                  <n-tag v-if="image.ai_room" size="small" type="info">{{ roomLabel(image.ai_room) }}</n-tag>
                  <n-tag v-if="image.ai_image_type" size="small">{{ imageTypeLabel(image.ai_image_type) }}</n-tag>
                </n-space>

                <div v-if="image.ai_description" style="font-size: 0.85em; color: #aaa">
                  {{ image.ai_description }}
                </div>

                <n-space v-if="image.ai_issues?.length" wrap>
                  <n-tag v-for="issue in image.ai_issues" :key="issue" size="small" type="warning">
                    {{ issue }}
                  </n-tag>
                </n-space>

                <div v-if="!image.ai_analyzed_at" style="color: #666; font-size: 0.85em">
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
