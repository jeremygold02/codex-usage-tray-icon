namespace CodexUsageTray
{
    internal enum ResetCreditRedemptionOutcome
    {
        Reset,
        AlreadyRedeemed,
        NothingToReset,
        NoCredit,
        Failed
    }

    internal sealed class ResetCreditRedemptionResult
    {
        public ResetCreditRedemptionOutcome Outcome { get; private set; }
        public string ErrorMessage { get; private set; }

        public bool IsSuccessful
        {
            get
            {
                return Outcome == ResetCreditRedemptionOutcome.Reset ||
                    Outcome == ResetCreditRedemptionOutcome.AlreadyRedeemed;
            }
        }

        private ResetCreditRedemptionResult(
            ResetCreditRedemptionOutcome outcome,
            string errorMessage)
        {
            Outcome = outcome;
            ErrorMessage = errorMessage;
        }

        public static ResetCreditRedemptionResult FromOutcome(
            ResetCreditRedemptionOutcome outcome)
        {
            return new ResetCreditRedemptionResult(outcome, null);
        }

        public static ResetCreditRedemptionResult FromError(string message)
        {
            return new ResetCreditRedemptionResult(
                ResetCreditRedemptionOutcome.Failed,
                string.IsNullOrWhiteSpace(message)
                    ? "The reset request failed."
                    : message.Trim());
        }
    }
}
