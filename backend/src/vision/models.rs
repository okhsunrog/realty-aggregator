use serde::{Deserialize, Serialize};

/// Оценка состояния квартиры
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionAssessment {
    /// bare_walls | needs_renovation | livable | good | excellent | unknown
    pub state: String,
    pub confidence: f64,
}

/// Тип ремонта
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenovationAssessment {
    /// none | cosmetic | standard | euro | designer | unknown
    #[serde(alias = "type")]
    pub renovation_type: String,
    pub confidence: f64,
}

/// Трёхзначная оценка с уверенностью (true/false/unknown)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriStateAssessment {
    #[serde(alias = "present")]
    pub value: TriStateValue,
    pub confidence: f64,
}

/// Значение: true, false или unknown
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TriStateValue {
    Bool(bool),
    Unknown(String), // "unknown"
}

impl TriStateValue {
    pub fn is_true(&self) -> bool {
        matches!(self, TriStateValue::Bool(true))
    }

    pub fn is_false(&self) -> bool {
        matches!(self, TriStateValue::Bool(false))
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, TriStateValue::Unknown(_))
    }

    pub fn as_option(&self) -> Option<bool> {
        match self {
            TriStateValue::Bool(v) => Some(*v),
            TriStateValue::Unknown(_) => None,
        }
    }
}

/// Общая оценка привлекательности (1-10)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreAssessment {
    pub value: i32,
    pub confidence: f64,
}

/// Счётчик использованных изображений
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagesUsedAssessment {
    pub value: i32,
    pub confidence: f64,
}

/// Полный результат vision-анализа квартиры
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionAnalysis {
    /// Состояние квартиры
    pub condition: ConditionAssessment,

    /// Тип ремонта
    pub renovation: RenovationAssessment,

    /// Пустая новостройка без отделки (ключевой параметр)
    pub is_empty_shell: TriStateAssessment,

    /// Наличие мебели
    pub furniture: TriStateAssessment,

    /// Наличие техники (холодильник, плита, стиралка)
    pub appliances: TriStateAssessment,

    /// Можно ли въехать сразу без ремонта
    pub move_in_ready: TriStateAssessment,

    /// Общая оценка привлекательности 1-10
    pub overall_score: ScoreAssessment,

    /// Количество использованных изображений
    #[serde(default)]
    pub images_used: Option<ImagesUsedAssessment>,

    /// Визуальные признаки, на которых основана оценка
    #[serde(default)]
    pub evidence: Vec<String>,

    /// Краткие заметки нейросети о квартире
    #[serde(default)]
    pub notes: String,

    /// Противоречия между фотографиями (если есть)
    #[serde(default)]
    pub contradictions: Vec<String>,
}

impl VisionAnalysis {
    /// Проверка, что анализ имеет достаточную уверенность
    pub fn is_confident(&self, threshold: f64) -> bool {
        self.condition.confidence >= threshold
            && self.renovation.confidence >= threshold
            && self.is_empty_shell.confidence >= threshold
    }

    /// Средняя уверенность по основным полям
    pub fn average_confidence(&self) -> f64 {
        let sum = self.condition.confidence
            + self.renovation.confidence
            + self.is_empty_shell.confidence
            + self.furniture.confidence
            + self.appliances.confidence
            + self.move_in_ready.confidence
            + self.overall_score.confidence;
        sum / 7.0
    }

    /// Это пустая новостройка без отделки?
    pub fn is_shell(&self) -> Option<bool> {
        self.is_empty_shell.value.as_option()
    }

    /// Готова ли квартира к заселению?
    pub fn is_ready(&self) -> Option<bool> {
        self.move_in_ready.value.as_option()
    }
}
