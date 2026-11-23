//! Progress and failure reporting for the list's bulk delete.
//!
//! Kept apart from the query/data plumbing in `ViewCore`: the report is a
//! self-contained value with its own invariants, and its tests are about the
//! wording the user sees, not about fetching.

/// Estado del borrado masivo.
///
/// Antes, `bulk_delete` disparaba las N peticiones en un bucle sin más: cada
/// una por su cuenta a través de `use_mutation`, que comparte **una sola**
/// señal de error. El `error.set(None)` de la petición siguiente borraba el
/// fallo de la anterior, así que un borrado parcial podía terminar sin dejar
/// rastro de lo que no se había borrado — y `mutating` volvía a `false` en
/// cuanto acababa la primera, no la última.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BulkProgress {
    /// Documentos que había que borrar al empezar.
    pub total: usize,
    /// Peticiones ya respondidas, fallen o no.
    pub done: usize,
    /// Los que no se pudieron borrar, con el motivo.
    pub failed: Vec<(String, String)>,
    /// `true` mientras el bucle está en marcha.
    pub running: bool,
    /// Fallo previo al bucle: no hubo runtime contra el que borrar. No es un
    /// documento que falló —no se intentó ninguno— y decirlo como si lo fuera
    /// sería el mismo tipo de mentira que el documento original.
    pub fatal: Option<String>,
}

impl BulkProgress {
    /// Una línea por documento que falló, para el aviso.
    pub fn failure_report(&self, i18n: &impl crate::i18n::Translate) -> Option<String> {
        // Nada se reporta mientras el bucle corre. `failed` crece documento a
        // documento, y un aviso que se reescribe bajo los ojos del usuario —que
        // además cambia de longitud— no informa de nada. El avance lo lleva la
        // línea de progreso; el aviso es para cuando ya no hay nada que hacer.
        if self.running {
            return None;
        }
        if let Some(fatal) = &self.fatal {
            return Some(i18n.t_with("components.bulk.fatal", &[("reason", fatal)]));
        }
        if self.failed.is_empty() {
            return None;
        }
        let lines: Vec<String> = self
            .failed
            .iter()
            .map(|(name, reason)| format!("{name}: {reason}"))
            .collect();
        Some(i18n.t_with(
            "components.bulk.partial",
            &[
                ("failed", &self.failed.len().to_string()),
                ("total", &self.total.to_string()),
                ("lines", &lines.join("; ")),
            ],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::BulkProgress;

    /// Un borrado completo no tiene nada que avisar: si lo tuviera, el usuario
    /// leería un error donde sólo se borró lo que pidió.
    fn es() -> crate::i18n::DeskI18n {
        crate::i18n::DeskI18n::from_catalog(
            "es",
            &serde_json::json!({
                "components.bulk.fatal": "No se pudo eliminar nada: {reason}",
                "components.bulk.partial": "No se pudieron eliminar {failed} de {total} documentos: {lines}",
            }),
        )
    }

    #[test]
    fn failure_report_is_absent_when_everything_was_deleted() {
        let p = BulkProgress {
            total: 3,
            done: 3,
            failed: Vec::new(),
            running: false,
            fatal: None,
        };
        assert_eq!(p.failure_report(&crate::i18n::DeskI18n::default()), None);
    }

    /// El aviso tiene que decir qué falló y por qué, no sólo que algo falló: los
    /// documentos que sobreviven son los que el usuario puede volver a intentar,
    /// y sólo él sabe si le importa la razón.
    #[test]
    fn failure_report_names_each_failed_document_with_its_reason() {
        let p = BulkProgress {
            total: 3,
            done: 3,
            failed: vec![
                ("TASK00001".to_string(), "No permission".to_string()),
                ("TASK00002".to_string(), "Locked".to_string()),
            ],
            running: false,
            fatal: None,
        };
        let report = p.failure_report(&es()).expect("hay fallos que avisar");
        assert!(report.contains("2 de 3"), "no dice cuántos: {report}");
        assert!(report.contains("TASK00001: No permission"), "{report}");
        assert!(report.contains("TASK00002: Locked"), "{report}");
    }

    /// Mientras el bucle corre no hay nada que reportar todavía: `failed` crece
    /// de uno en uno y el banner debe esperar a tener el conjunto, o mostraría
    /// un aviso que se reescribe —y cambia de longitud— bajo los ojos del
    /// usuario. El avance lo lleva la línea de progreso.
    #[test]
    fn failure_report_stays_absent_while_the_loop_is_running() {
        let p = BulkProgress {
            total: 2,
            done: 1,
            failed: vec![("TASK00001".to_string(), "No permission".to_string())],
            running: true,
            fatal: None,
        };
        assert_eq!(p.failure_report(&crate::i18n::DeskI18n::default()), None);
    }

    /// Un fallo previo al bucle no es un documento que falló: no se intentó
    /// ninguno. El aviso tiene que distinguir los dos casos, porque el segundo
    /// no se arregla reintentando.
    #[test]
    fn failure_report_separates_a_fatal_stop_from_per_document_failures() {
        let p = BulkProgress {
            total: 4,
            done: 0,
            failed: Vec::new(),
            running: false,
            fatal: Some("no runtime".to_string()),
        };
        let report = p.failure_report(&es()).expect("hay algo que avisar");
        assert!(report.contains("No se pudo eliminar nada"), "{report}");
        assert!(report.contains("no runtime"), "{report}");
        assert!(!report.contains("0 de 4"), "{report}");
    }
}
