//! Kind of policy the cabinet controller is following.
//!
//! [`AppMode`] only names the policy. The mode structs own the transitions.

/// Kind of policy the cabinet controller is following.
///
/// A pure value. It carries the behavior name and the yellow-band title.
/// Mode structs apply the relays and the transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// The sample rack is incomplete or dead.
    ///
    /// Behavior name: `WAIT_PROBE`.
    #[default]
    WaitProbe,
    /// Both relays are off. Compressor rest may still be running.
    ///
    /// Behavior name: `NOMINAL`.
    Nominal,
    /// All N samples have vapor-pressure deficit under 0.6 kPa.
    ///
    /// Dehumidifier on, fridge off. This policy has no deadline.
    /// Behavior name: `VPD_REGULATION`.
    VpdRegulation,
    /// Rest is clear, temperature is above 19°C, and vapor-pressure deficit is above 0.8 kPa.
    ///
    /// Fridge on. The dehumidifier latches on once all N samples are at or above 65% humidity.
    /// Behavior name: `TEMP_ENFORCING`.
    TempEnforcing,
    /// All N samples have humidity at or above 70%.
    ///
    /// Fridge off, dehumidifier on.
    /// Behavior name: `HR_ENFORCING`.
    HrEnforcing,
    /// `TEMP_ENFORCING` ran for 15 minutes or `HR_ENFORCING` ran for 30 minutes.
    ///
    /// Both relays off. Only a reboot leaves this policy.
    /// Behavior name: `FAILURE_STOP`.
    FailureStop,
}

impl AppMode {
    /// Stable label used in logs: `WAIT_PROBE`, `NOMINAL`, and the other policy names.
    pub const fn behavior_name(self) -> &'static str {
        match self {
            Self::WaitProbe => "WAIT_PROBE",
            Self::Nominal => "NOMINAL",
            Self::VpdRegulation => "VPD_REGULATION",
            Self::TempEnforcing => "TEMP_ENFORCING",
            Self::HrEnforcing => "HR_ENFORCING",
            Self::FailureStop => "FAILURE_STOP",
        }
    }

    /// Yellow-band title. The longest title is 16 characters.
    pub const fn title(self) -> &'static str {
        match self {
            Self::WaitProbe => "ÉCHEC SONDE",
            Self::Nominal => "NOMINAL",
            Self::VpdRegulation => "RÉGULATION HYGR.",
            Self::TempEnforcing => "FORÇAGE TEMP.",
            Self::HrEnforcing => "FORÇAGE HYGRO.",
            Self::FailureStop => "ÉCHEC / ARRÊT",
        }
    }
}
