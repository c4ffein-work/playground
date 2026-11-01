// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {CharityEvent} from "./CharityEvent.sol";

interface IEventRegistry {
    function isEvent(address) external view returns (bool);
}

/// @title GivingVaults
/// @notice Recurring giving with an exit. Each donor keeps a balance here and
/// a monthly plan. Once a month anyone can trigger the payment, which is
/// split across events by the donor's allocation, or by the allocation of a
/// curator the donor follows. Changing either takes one transaction and
/// applies from the next payment; unspent balance can be withdrawn any time.
///
/// Organizations never hold more than a month of a donor's money in advance,
/// so "I'll give elsewhere" is a real threat to a bad actor.
contract GivingVaults is ReentrancyGuard {
    using SafeERC20 for IERC20;

    /// Minimum gap between payments. Shorter than a month so a keeper can
    /// pay on a fixed calendar day without drifting.
    uint64 public constant PERIOD = 28 days;
    uint16 public constant BPS = 10_000;

    struct Allocation {
        address[] targets; // CharityEvents
        uint16[] bps; // shares, sum to 10_000
    }

    struct Plan {
        uint256 balance;
        uint256 monthly;
        uint64 lastPaid;
    }

    IEventRegistry public immutable events;
    mapping(address => mapping(address => Plan)) public plans; // donor => token => plan
    mapping(address => Allocation) private _allocations; // donor or curator => split
    mapping(address => address) public following; // donor => curator (0 = own split)

    event Deposited(address indexed donor, address indexed token, uint256 amount);
    event Withdrawn(address indexed donor, address indexed token, uint256 amount);
    event MonthlySet(address indexed donor, address indexed token, uint256 monthly);
    event AllocationSet(address indexed who, address[] targets, uint16[] bps);
    event Followed(address indexed donor, address indexed curator);
    event Paid(address indexed donor, address indexed token, address indexed target, uint256 amount);
    /// A target was closed or rejected the token: that share stays in the vault.
    event Skipped(address indexed donor, address indexed token, address indexed target, uint256 amount);

    error BadAllocation();
    error NotDue();
    error NothingToPay();
    error ZeroAmount();
    error InsufficientBalance();

    constructor(IEventRegistry events_) {
        events = events_;
    }

    // --------------------------------------------------------- donor setup

    function deposit(address token, uint256 amount) external nonReentrant {
        if (amount == 0) revert ZeroAmount();
        IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        plans[msg.sender][token].balance += amount;
        emit Deposited(msg.sender, token, amount);
    }

    /// @notice Take back money that hasn't been given yet.
    function withdraw(address token, uint256 amount) external nonReentrant {
        Plan storage p = plans[msg.sender][token];
        if (amount > p.balance) revert InsufficientBalance();
        p.balance -= amount;
        IERC20(token).safeTransfer(msg.sender, amount);
        emit Withdrawn(msg.sender, token, amount);
    }

    function setMonthly(address token, uint256 monthly) external {
        plans[msg.sender][token].monthly = monthly;
        emit MonthlySet(msg.sender, token, monthly);
    }

    /// @notice Set your own split. Curators use the same call to publish theirs.
    /// Setting your own split also stops following a curator.
    function setAllocation(address[] calldata targets, uint16[] calldata bps) external {
        if (targets.length == 0 || targets.length != bps.length) revert BadAllocation();
        uint256 sum;
        for (uint256 i; i < targets.length; ++i) {
            if (!events.isEvent(targets[i]) || bps[i] == 0) revert BadAllocation();
            sum += bps[i];
        }
        if (sum != BPS) revert BadAllocation();
        _allocations[msg.sender] = Allocation(targets, bps);
        emit AllocationSet(msg.sender, targets, bps);
        if (following[msg.sender] != address(0)) {
            following[msg.sender] = address(0);
            emit Followed(msg.sender, address(0));
        }
    }

    /// @notice Let a curator decide your split. Their changes apply to you
    /// from your next payment. Follow address(0) to go back to your own split.
    function follow(address curator) external {
        if (curator == msg.sender) revert BadAllocation();
        following[msg.sender] = curator;
        emit Followed(msg.sender, curator);
    }

    // ------------------------------------------------------------- paying

    /// @notice Pay this month's gift. Anyone can call it (e.g. a keeper bot).
    function pay(address donor, address token) external nonReentrant {
        Plan storage p = plans[donor][token];
        if (p.lastPaid != 0 && block.timestamp < p.lastPaid + PERIOD) revert NotDue();
        uint256 amount = p.monthly < p.balance ? p.monthly : p.balance;
        if (amount == 0) revert NothingToPay();
        Allocation storage a = _allocationOf(donor);
        if (a.targets.length == 0) revert BadAllocation();

        p.lastPaid = uint64(block.timestamp);
        uint256 left = amount;
        for (uint256 i; i < a.targets.length; ++i) {
            uint256 part = i == a.targets.length - 1 ? left : (amount * a.bps[i]) / BPS;
            left -= part;
            if (part == 0) continue;
            address target = a.targets[i];
            if (_accepts(target, token)) {
                p.balance -= part;
                IERC20(token).forceApprove(target, part);
                CharityEvent(target).donateFor(donor, token, part);
                emit Paid(donor, token, target, part);
            } else {
                emit Skipped(donor, token, target, part);
            }
        }
    }

    function _accepts(address target, address token) private view returns (bool) {
        CharityEvent e = CharityEvent(target);
        return !e.closed() && e.isAcceptedToken(token);
    }

    // -------------------------------------------------------------- views

    /// @notice The split that applies to `donor` right now.
    function _allocationOf(address donor) internal view returns (Allocation storage) {
        address c = following[donor];
        return c == address(0) ? _allocations[donor] : _allocations[c];
    }

    function allocation(address who) external view returns (address[] memory targets, uint16[] memory bps) {
        Allocation storage a = _allocationOf(who);
        return (a.targets, a.bps);
    }

    function ownAllocation(address who) external view returns (address[] memory targets, uint16[] memory bps) {
        Allocation storage a = _allocations[who];
        return (a.targets, a.bps);
    }
}
