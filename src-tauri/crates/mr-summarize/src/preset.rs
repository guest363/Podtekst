use serde::{Deserialize, Serialize};

use crate::error::SummaryError;

pub const CATALOG_VERSION: u32 = 1;
pub const PRESET_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionRole {
    Plain,
    Decision,
    Task,
    Status,
}

impl SectionRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Decision => "decision",
            Self::Task => "task",
            Self::Status => "status",
        }
    }

    pub fn is_task(self) -> bool {
        matches!(self, Self::Task)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SectionSpec {
    pub id: &'static str,
    pub title_ru: &'static str,
    pub title_en: &'static str,
    pub role: SectionRole,
}

const fn sec(
    id: &'static str,
    ru: &'static str,
    en: &'static str,
    role: SectionRole,
) -> SectionSpec {
    SectionSpec {
        id,
        title_ru: ru,
        title_en: en,
        role,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub label_ru: &'static str,
    pub label_en: &'static str,
    pub short_ru: &'static str,
    pub short_en: &'static str,
    pub caution_ru: &'static str,
    pub caution_en: &'static str,
    pub sensitive: bool,
    pub sections: &'static [SectionSpec],
    pub speaker_primary: &'static [&'static str],
    pub speaker_secondary: &'static [&'static str],
    pub prompt_version: &'static str,
}

const MEETING_SECTIONS: &[SectionSpec] = &[
    sec("brief", "Кратко", "Brief", SectionRole::Plain),
    sec("decisions", "Решения", "Decisions", SectionRole::Decision),
    sec("tasks", "Задачи", "Tasks", SectionRole::Task),
    sec(
        "questions",
        "Открытые вопросы",
        "Open questions",
        SectionRole::Plain,
    ),
];
const SCRUM_SECTIONS: &[SectionSpec] = &[
    sec("brief", "Кратко", "Brief", SectionRole::Plain),
    sec("done", "Сделано", "Done", SectionRole::Status),
    sec(
        "in_progress",
        "В работе",
        "In progress",
        SectionRole::Status,
    ),
    sec(
        "blockers",
        "Блокеры и риски",
        "Blockers and risks",
        SectionRole::Status,
    ),
    sec("decisions", "Решения", "Decisions", SectionRole::Decision),
    sec("tasks", "Задачи", "Tasks", SectionRole::Task),
];
const LEGAL_SECTIONS: &[SectionSpec] = &[
    sec("substance", "Суть", "Substance", SectionRole::Plain),
    sec(
        "parties",
        "Стороны и роли",
        "Parties and roles",
        SectionRole::Plain,
    ),
    sec(
        "facts",
        "Факты и доводы",
        "Facts and arguments",
        SectionRole::Plain,
    ),
    sec(
        "norms",
        "Нормы и ссылки",
        "Norms and references",
        SectionRole::Plain,
    ),
    sec(
        "questions",
        "Вопросы к разбору",
        "Questions for review",
        SectionRole::Plain,
    ),
    sec(
        "next_steps",
        "Следующие шаги",
        "Next steps",
        SectionRole::Task,
    ),
];
const LECTURE_SECTIONS: &[SectionSpec] = &[
    sec(
        "topic",
        "Тема и тезис",
        "Topic and thesis",
        SectionRole::Plain,
    ),
    sec(
        "concepts",
        "Ключевые понятия",
        "Key concepts",
        SectionRole::Plain,
    ),
    sec("structure", "Структура", "Structure", SectionRole::Plain),
    sec("examples", "Примеры", "Examples", SectionRole::Plain),
    sec(
        "takeaways",
        "Что запомнить",
        "Takeaways",
        SectionRole::Plain,
    ),
    sec("questions", "Вопросы", "Questions", SectionRole::Plain),
];
const PSYCHOLOGY_SECTIONS: &[SectionSpec] = &[
    sec(
        "topics",
        "Темы сессии",
        "Session topics",
        SectionRole::Plain,
    ),
    sec(
        "client_words",
        "Слова клиента",
        "Client's words",
        SectionRole::Plain,
    ),
    sec(
        "observations",
        "Наблюдения и гипотезы",
        "Observations and hypotheses",
        SectionRole::Plain,
    ),
    sec(
        "agreements",
        "Договорённости",
        "Agreements",
        SectionRole::Decision,
    ),
    sec(
        "next_session",
        "К следующей сессии",
        "Next session",
        SectionRole::Task,
    ),
    sec(
        "open_topics",
        "Открытые темы",
        "Open topics",
        SectionRole::Plain,
    ),
];
const PERSONAL_SECTIONS: &[SectionSpec] = &[
    sec(
        "about",
        "О чём запись",
        "About this recording",
        SectionRole::Plain,
    ),
    sec(
        "thoughts",
        "Мысли и выводы",
        "Thoughts and conclusions",
        SectionRole::Plain,
    ),
    sec("decisions", "Решения", "Decisions", SectionRole::Decision),
    sec("todo", "Сделать", "To do", SectionRole::Task),
    sec(
        "questions",
        "Открытые вопросы",
        "Open questions",
        SectionRole::Plain,
    ),
];
const UNIVERSAL_SECTIONS: &[SectionSpec] = &[
    sec("brief", "Кратко", "Brief", SectionRole::Plain),
    sec("main", "Главное", "Main points", SectionRole::Plain),
    sec(
        "decisions",
        "Решения и договорённости",
        "Decisions and agreements",
        SectionRole::Decision,
    ),
    sec("tasks", "Задачи", "Tasks", SectionRole::Task),
    sec(
        "questions",
        "Открытые вопросы",
        "Open questions",
        SectionRole::Plain,
    ),
];

pub static PRESETS: [Preset; 7] = [
    Preset {
        id: "meeting",
        label_ru: "Встреча",
        label_en: "Meeting",
        short_ru: "Рабочая встреча",
        short_en: "Work meeting",
        caution_ru: "Проверьте имена, сроки и формулировки задач по ссылкам на реплики.",
        caution_en: "Check names, deadlines, and task wording against the linked utterances.",
        sensitive: false,
        sections: MEETING_SECTIONS,
        speaker_primary: &[],
        speaker_secondary: &[],
        prompt_version: "meeting-v2",
    },
    Preset {
        id: "scrum",
        label_ru: "Скрам",
        label_en: "Scrum",
        short_ru: "Стендап и спринт",
        short_en: "Standup and sprint",
        caution_ru: "Сверьте статусы и блокеры с исходными репликами.",
        caution_en: "Verify statuses and blockers against the source utterances.",
        sensitive: false,
        sections: SCRUM_SECTIONS,
        speaker_primary: &[],
        speaker_secondary: &[],
        prompt_version: "scrum-v2",
    },
    Preset {
        id: "legal",
        label_ru: "Юридическая встреча",
        label_en: "Legal meeting",
        short_ru: "Заседание или консультация",
        short_en: "Hearing or consultation",
        caution_ru: "Это черновик, не юридическая консультация. Проверьте нормы, номера и формулировки по исходным репликам.",
        caution_en: "This is a draft, not legal advice. Verify norms, numbers, and wording against the source utterances.",
        sensitive: true,
        sections: LEGAL_SECTIONS,
        speaker_primary: &["судья", "юрист", "judge", "lawyer"],
        speaker_secondary: &[],
        prompt_version: "legal-v2",
    },
    Preset {
        id: "lecture",
        label_ru: "Лекция",
        label_en: "Lecture",
        short_ru: "Лекция или вебинар",
        short_en: "Lecture or webinar",
        caution_ru: "Определения и примеры приведены по словам лектора.",
        caution_en: "Definitions and examples follow the lecturer's words.",
        sensitive: false,
        sections: LECTURE_SECTIONS,
        speaker_primary: &["лектор", "преподаватель", "lecturer", "teacher"],
        speaker_secondary: &["студент", "student"],
        prompt_version: "lecture-v2",
    },
    Preset {
        id: "psychology",
        label_ru: "Психологическая сессия",
        label_en: "Psychology session",
        short_ru: "Заметки о разговоре",
        short_en: "Conversation notes",
        caution_ru: "Это заметки о разговоре, не медицинская консультация и не диагноз. Проверьте формулировки.",
        caution_en: "These are conversation notes, not medical advice or a diagnosis. Check the wording.",
        sensitive: true,
        sections: PSYCHOLOGY_SECTIONS,
        speaker_primary: &["клиент", "client"],
        speaker_secondary: &["терапевт", "психолог", "therapist", "psychologist"],
        prompt_version: "psychology-v2",
    },
    Preset {
        id: "personal",
        label_ru: "Личные заметки",
        label_en: "Personal notes",
        short_ru: "Монолог или заметка",
        short_en: "Monologue or note",
        caution_ru: "Личные заметки. Проверьте, что смысл не искажён.",
        caution_en: "Personal notes. Check that the meaning is not distorted.",
        sensitive: false,
        sections: PERSONAL_SECTIONS,
        speaker_primary: &[],
        speaker_secondary: &[],
        prompt_version: "personal-v2",
    },
    Preset {
        id: "universal",
        label_ru: "Универсальный",
        label_en: "Universal",
        short_ru: "Смешанный формат",
        short_en: "Mixed format",
        caution_ru: "Проверьте итоги по ссылкам на реплики.",
        caution_en: "Check the summary against the linked utterances.",
        sensitive: false,
        sections: UNIVERSAL_SECTIONS,
        speaker_primary: &[],
        speaker_secondary: &[],
        prompt_version: "universal-v2",
    },
];

pub fn all() -> &'static [Preset] {
    &PRESETS
}

pub fn get(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

pub fn require(id: &str) -> Result<&'static Preset, SummaryError> {
    get(id).ok_or_else(|| SummaryError::InvalidPreset(id.to_owned()))
}

pub fn section(preset: &Preset, id: &str) -> Option<&'static SectionSpec> {
    preset.sections.iter().find(|s| s.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    Short,
    Normal,
    Long,
}

impl Detail {
    pub const DEFAULT: Self = Self::Normal;

    pub fn parse(value: &str) -> Result<Self, SummaryError> {
        match value {
            "short" => Ok(Self::Short),
            "normal" => Ok(Self::Normal),
            "long" => Ok(Self::Long),
            other => Err(SummaryError::InvalidDetail(other.to_owned())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Normal => "normal",
            Self::Long => "long",
        }
    }

    pub fn max_tokens(self) -> u32 {
        match self {
            Self::Short => 512,
            Self::Normal => 768,
            Self::Long => 1024,
        }
    }

    pub fn max_items(self) -> usize {
        match self {
            Self::Short => 5,
            Self::Normal => 8,
            Self::Long => 12,
        }
    }

    pub fn map_bullets(self) -> usize {
        match self {
            Self::Short => 3,
            Self::Normal => 5,
            Self::Long => 7,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_seven_presets_universal_last() {
        assert_eq!(all().len(), 7);
        assert_eq!(all()[6].id, "universal");
        let mut ids: Vec<_> = all().iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 7);
    }

    #[test]
    fn sensitive_only_legal_and_psychology() {
        for p in all() {
            assert_eq!(
                p.sensitive,
                matches!(p.id, "legal" | "psychology"),
                "{}",
                p.id
            );
        }
    }

    #[test]
    fn sections_and_roles_follow_catalog() {
        let legal = get("legal").unwrap();
        assert_eq!(legal.sections[0].id, "substance");
        assert_eq!(legal.sections[0].role, SectionRole::Plain);
        assert_eq!(legal.sections[5].id, "next_steps");
        assert_eq!(legal.sections[5].role, SectionRole::Task);
        let lecture = get("lecture").unwrap();
        assert!(lecture
            .sections
            .iter()
            .all(|s| s.role == SectionRole::Plain));
        let scrum = get("scrum").unwrap();
        assert_eq!(scrum.sections[1].role, SectionRole::Status);
        assert_eq!(scrum.sections[4].role, SectionRole::Decision);
        assert_eq!(scrum.sections[5].role, SectionRole::Task);
    }

    #[test]
    fn prompt_versions_are_pinned() {
        for p in all() {
            assert_eq!(p.prompt_version, format!("{}-v2", p.id));
            assert!(get(p.id).is_some());
        }
        assert!(require("other").is_err());
    }

    #[test]
    fn detail_overrides_duration() {
        assert_eq!(Detail::Short.max_tokens(), 512);
        assert_eq!(Detail::Normal.max_tokens(), 768);
        assert_eq!(Detail::Long.max_tokens(), 1024);
        assert!(matches!(Detail::parse("long"), Ok(Detail::Long)));
    }
}
