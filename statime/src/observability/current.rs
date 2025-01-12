use crate::datastructures::datasets::InternalCurrentDS;

/// A concrete implementation of the PTP Current dataset (IEEE1588-2019 section
/// 8.2.2)
#[derive(Debug, Default, Copy, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CurrentDS {
    /// See *IEEE1588-2019 section 8.2.2.2*.
    pub steps_removed: u16,
    /// See *IEEE1588-2019 section 8.2.2.3*.
    pub offset_from_master: i128,
    /// See *IEEE1588-2019 section 8.2.2.4*.
    pub mean_delay: i128,
}

impl From<&InternalCurrentDS> for CurrentDS {
    fn from(v: &InternalCurrentDS) -> Self {
        Self {
            steps_removed: v.steps_removed,
            offset_from_master: v.offset_from_master.nanos_rounded(),
            mean_delay: v.mean_delay.nanos_rounded(),
        }
    }
}
