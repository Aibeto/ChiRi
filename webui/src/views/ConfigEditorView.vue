<script setup lang="ts">
import { ref, onMounted, computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { Bridge } from '@/utils/bridge';
import { toast } from '@/kernelsu';

const { t } = useI18n();

const loading = ref(false);
// 生效配置文件的 meta 段（抬头信息）：配置名/作者/日志语言/日志等级，仅展示
const meta = ref<Record<string, any>>({});
// 生效配置文件相对 config 目录的路径（如 "8550/config.yaml"，非处理器时为 "config.yaml"）
const activeConfig = ref('config.yaml');

const showLoglevelSheet = ref(false);

const loglevelActions = computed(() => [
  { name: t('loglevel_off'), level: 'OFF' },
  { name: t('loglevel_error'), level: 'ERROR' },
  { name: t('loglevel_warn'), level: 'WARN' },
  { name: t('loglevel_info'), level: 'INFO' },
  { name: t('loglevel_debug'), level: 'DEBUG' },
  { name: t('loglevel_trace'), level: 'TRACE' }
]);

// 当前日志等级的中文/英文标签
const loglevelLabel = computed(() => {
  const lv = String(meta.value.loglevel || 'INFO').toUpperCase();
  const hit = loglevelActions.value.find(a => a.level === lv);
  return hit ? hit.name : lv;
});

// 开发记录开关（meta.dev_record）：开启后守护进程向 devimp/ 写按核调度诊断日志
const devRecord = ref(Boolean(meta.value.dev_record));

// v-model 在 @change 触发前已把 devRecord 翻转为新值，失败回滚用 !on（不能取 prev）
const onDevRecordChange = async (on: boolean) => {
  try {
    await Bridge.setDevRecord(on);
  } catch (e) {
    devRecord.value = !on;
    toast(t('save_failed'));
  }
};

// 日志语言标签（en/zh → 本地化文案）
const languageLabel = computed(() => {
  const lang = String(meta.value.language || '').toLowerCase();
  if (lang === 'zh') return t('lang_zh');
  if (lang === 'en') return t('lang_en');
  return meta.value.language || '-';
});

const loadData = async () => {
  loading.value = true;
  try {
    const [m, name] = await Promise.all([
      Bridge.getConfigMeta(),
      Bridge.getActiveConfigName()
    ]);
    meta.value = m || {};
    activeConfig.value = name;
    devRecord.value = Boolean(meta.value.dev_record);
  } catch (e) {
    toast(t('load_failed'));
  } finally {
    loading.value = false;
  }
};

onMounted(loadData);

// 切换日志等级：写入生效 config.yaml 的 meta.loglevel，守护进程热重载即时生效
const onSelectLoglevel = async (a: any) => {
  showLoglevelSheet.value = false;
  const prev = meta.value.loglevel;
  meta.value.loglevel = a.level;
  try {
    await Bridge.setLogLevel(a.level);
  } catch (e) {
    meta.value.loglevel = prev;
    toast(t('save_failed'));
  }
};
</script>

<template>
  <div class="config-info">
    <van-nav-bar :title="t('config_info')" left-arrow @click-left="$router.back()" fixed placeholder>
      <template #right><van-icon name="replay" size="18" @click="loadData" /></template>
    </van-nav-bar>

    <van-loading v-if="loading" class="loading-center" vertical>{{ t('loading') }}</van-loading>

    <div v-else class="info-content">
      <div class="section-title">{{ t('config_info') }}</div>
      <van-cell-group inset :border="false">
        <van-cell :title="t('active_config_file')" :value="activeConfig" />
        <van-cell :title="t('config_name')" :value="meta.name || '-'" />
        <van-cell :title="t('author')" :value="meta.author || '-'" />
        <van-cell :title="t('log_language')" :value="languageLabel" />
        <van-cell :title="t('log_level')" :value="loglevelLabel" is-link clickable @click="showLoglevelSheet = true" />
        <van-cell center :title="t('dev_record')" :label="t('dev_record_hint')">
          <template #right-icon>
            <van-switch v-model="devRecord" size="22" @change="onDevRecordChange" />
          </template>
        </van-cell>
      </van-cell-group>

      <div class="hint">{{ t('config_info_hint') }}</div>
    </div>

    <van-action-sheet v-model:show="showLoglevelSheet" :actions="loglevelActions" :cancel-text="t('cancel')"
      @select="onSelectLoglevel" />
  </div>
</template>

<style scoped>
.config-info {
  min-height: 100vh;
  background: #f7f8fa;
}

.loading-center {
  padding-top: 100px;
}

.info-content {
  padding: 12px 0 24px;
}

.section-title {
  margin: 20px 16px 10px;
  font-size: 14px;
  color: #969799;
  font-weight: 500;
}

.hint {
  margin: 16px;
  padding: 10px 12px;
  border-radius: 8px;
  background: #fff;
  color: #969799;
  font-size: 12px;
  line-height: 1.6;
}
</style>
