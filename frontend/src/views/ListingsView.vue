<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import {
  NGrid,
  NGi,
  NSpin,
  NPagination,
  NSpace,
  NStatistic,
  NCard,
  NInputNumber,
  NButton,
  NSwitch,
  NEmpty,
} from 'naive-ui'
import { useListingsStore } from '@/stores/listings'
import ListingCard from '@/components/ListingCard.vue'
import { fetchListingImages, type ListingImage } from '@/api'

const store = useListingsStore()
const router = useRouter()

// Cache first image for each listing
const listingImages = ref<Map<number, string | null>>(new Map())

async function loadFirstImages() {
  for (const listing of store.listings) {
    if (!listingImages.value.has(listing.id)) {
      try {
        const images = await fetchListingImages(listing.id)
        const firstImage = images.find(img => img.s3_key && img.status.toLowerCase() === 'downloaded')
        listingImages.value.set(listing.id, firstImage?.s3_key || null)
      } catch {
        listingImages.value.set(listing.id, null)
      }
    }
  }
}

onMounted(async () => {
  await Promise.all([store.loadStats(), store.loadListings()])
  loadFirstImages()
})

async function handlePageChange(page: number) {
  await store.loadListings(page)
  loadFirstImages()
}

function handleListingClick(listing: any) {
  router.push({ name: 'listing-detail', params: { id: listing.id } })
}

// Filter form
const minPrice = ref<number | undefined>()
const maxPrice = ref<number | undefined>()
const rooms = ref<number | undefined>()
const analyzedOnly = ref(false)
const hideShell = ref(false)
const activeOnly = ref(true)  // По умолчанию показываем только активные

async function applyFilters() {
  store.setFilters({
    minPrice: minPrice.value,
    maxPrice: maxPrice.value,
    rooms: rooms.value,
    analyzedOnly: analyzedOnly.value,
    hideShell: hideShell.value,
    activeOnly: activeOnly.value,
  })
  loadFirstImages()
}

function clearFilters() {
  minPrice.value = undefined
  maxPrice.value = undefined
  rooms.value = undefined
  analyzedOnly.value = false
  hideShell.value = false
  activeOnly.value = true
  store.setFilters({
    minPrice: undefined,
    maxPrice: undefined,
    rooms: undefined,
    analyzedOnly: false,
    hideShell: false,
    activeOnly: true,
  })
}
</script>

<template>
  <n-space vertical size="large">
    <!-- Stats -->
    <n-grid :cols="4" :x-gap="16" v-if="store.stats">
      <n-gi>
        <n-card size="small">
          <n-statistic label="Всего объявлений" :value="store.stats.total_listings" />
        </n-card>
      </n-gi>
      <n-gi>
        <n-card size="small">
          <n-statistic label="Изображений" :value="store.stats.images.downloaded" />
        </n-card>
      </n-gi>
      <n-gi>
        <n-card size="small">
          <n-statistic label="Проанализировано фото" :value="store.stats.analysis.images_analyzed" />
        </n-card>
      </n-gi>
      <n-gi>
        <n-card size="small">
          <n-statistic label="Проанализировано объявлений" :value="store.stats.analysis.listings_analyzed" />
        </n-card>
      </n-gi>
    </n-grid>

    <!-- Filters -->
    <n-card title="Фильтры" size="small">
      <n-space align="center">
        <n-input-number
          v-model:value="minPrice"
          placeholder="Цена от"
          :min="0"
          :step="100000"
          clearable
          style="width: 150px"
        />
        <n-input-number
          v-model:value="maxPrice"
          placeholder="Цена до"
          :min="0"
          :step="100000"
          clearable
          style="width: 150px"
        />
        <n-input-number
          v-model:value="rooms"
          placeholder="Комнат"
          :min="1"
          :max="10"
          clearable
          style="width: 120px"
        />
        <n-space align="center">
          <n-switch v-model:value="analyzedOnly" />
          <span>Только с анализом</span>
        </n-space>
        <n-space align="center">
          <n-switch v-model:value="hideShell" />
          <span>Скрыть без отделки</span>
        </n-space>
        <n-space align="center">
          <n-switch v-model:value="activeOnly" />
          <span>Только активные</span>
        </n-space>
        <n-button type="primary" @click="applyFilters">Применить</n-button>
        <n-button @click="clearFilters">Сбросить</n-button>
      </n-space>
    </n-card>

    <!-- Listings Grid -->
    <n-spin :show="store.loading">
      <n-grid v-if="store.listings.length > 0" :cols="4" :x-gap="16" :y-gap="16" responsive="screen" :cols-xs="1" :cols-s="2" :cols-m="3" :cols-l="4">
        <n-gi v-for="listing in store.listings" :key="listing.id">
          <ListingCard
            :listing="listing"
            :first-image-key="listingImages.get(listing.id)"
            @click="handleListingClick"
          />
        </n-gi>
      </n-grid>
      <n-empty v-else-if="!store.loading" description="Объявления не найдены" />
    </n-spin>

    <!-- Pagination -->
    <n-space justify="center" v-if="store.totalPages > 1">
      <n-pagination
        :page="store.currentPage"
        :page-count="store.totalPages"
        :page-slot="7"
        @update:page="handlePageChange"
      />
    </n-space>
  </n-space>
</template>
